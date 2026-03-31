# IoQueue Fix Plan: TCP Sequence Mismatch (rc=412)

## Executive Summary

The `ioqueue_stress_test` fails because our Rust ioqueue implementation uses a **non-recursive `std::sync::Mutex`** with a **`processing` flag** to approximate pjproject's concurrency control. This fundamentally cannot replicate pjproject's `allow_concurrent=false` semantics, where a **recursive per-key mutex is held through the entire callback invocation**, serializing all I/O operations on a given key.

---

## 1. Exact Root Cause

### The Test's Requirement

The stress test (`ioq_stress_test.c`) creates TCP socket pairs and spawns 16 threads calling `pj_ioqueue_poll()` concurrently. Each `on_write_complete` callback:

1. Acquires `grp_lock`
2. Fills `send_buf` with sequential integers: `buf[i] = test->state.cnt[CLIENT]++`
3. Calls `pj_ioqueue_send()` to send the next chunk

Each `on_read_complete` callback verifies received integers match: `expected == test->state.cnt[SERVER]++`. If not → **retcode 412**.

### Why Our Code Fails

**Bug 1: `processing` flag ≠ `allow_concurrent=false`**

Our Rust code does:
```
lock(key.mutex)
set processing = true
unlock(key.mutex)          // ← MUTEX RELEASED HERE
fire_callback(on_write_complete)
lock(key.mutex)
set processing = false
unlock(key.mutex)
```

pjproject with `allow_concurrent=false` does:
```
lock(key.recursive_mutex)  // trylock actually
do_send()
// DO NOT UNLOCK — keep holding
fire_callback(on_write_complete)
unlock(key.recursive_mutex) // ← RELEASED AFTER CALLBACK
```

During our callback, the key mutex is **not held**. Another poll thread can:
- Lock the key
- See a writable socket
- Do a `send()` system call
- This send may interleave with the data the callback is about to queue via `pj_ioqueue_send()`

**Bug 2: Non-recursive mutex causes deadlock or avoidance**

pjproject uses `pj_lock_create_recursive_mutex()` for per-key locks. The callback calls `pj_ioqueue_send()` → `pj_ioqueue_lock_key()`. With `allow_concurrent=false`, the lock is already held by the same thread. A recursive mutex allows re-entry; `std::sync::Mutex` would deadlock.

Our code works around this by releasing the mutex before the callback, but this creates the race window that breaks ordering.

**Bug 3: Fast-path send ordering violation**

In `pj_ioqueue_send()`, pjproject does a speculative `pj_list_empty(&key->write_list)` check **without the lock**, then tries an immediate `send()`. This is safe in pjproject because:
- With `allow_concurrent=false`, the callback thread holds the key lock, so no other thread can be in dispatch
- The recursive mutex lets the callback thread re-enter `pj_ioqueue_send()` and see the list state correctly

In our code, the callback thread does NOT hold the key lock, so a concurrent poll thread's dispatch can race with the callback's `ioqueue_send_impl()` fast-path.

### The Race Scenario (step by step)

1. **Thread A** (poll): locks key, sends queued data for write_op #N, removes it from queue, **releases lock**, fires `on_write_complete`
2. **Thread A** (callback): acquires grp_lock, fills buf with counter value 42, calls `ioqueue_send_impl()`
3. **Thread B** (poll): sees socket is writable, locks key (lock is free!), tries to dispatch — but there's nothing in write queue yet, so it skips
4. **Thread A** (in `ioqueue_send_impl`): fast-path check — write queue is empty → does immediate `send()` of value 42 ✓
5. Meanwhile, **Thread A**'s callback returns, and the poll loop continues

BUT consider this interleaving:

1. **Thread A** (poll): dispatches write_op #N, **releases lock**, fires callback
2. **Thread A** (callback): fills buf with counter=42, enters `ioqueue_send_impl`
3. **Thread B** (poll): locks key, sees socket writable, but write queue empty, unlocks
4. **Thread C** (different key's callback on same socket — or a timer): calls `ioqueue_send_impl` for value 43 (counter incremented)
5. **Thread C**'s fast-path send goes first → sends 43
6. **Thread A**'s fast-path send goes second → sends 42
7. **Receiver gets 43, 42** → rc=412!

The grp_lock prevents the counter from being corrupted, but it does NOT prevent the actual `send()` syscalls from being reordered when the key mutex isn't held.

---

## 2. pjproject's Exact Locking Protocol

### Lock Types

| Lock | Type | Scope | Purpose |
|------|------|-------|---------|
| `ioqueue->lock` | Simple mutex | Per-ioqueue | Protects fd_sets, active_list |
| `key->lock` | **Recursive mutex** | Per-key | Serializes I/O ops on a key |
| `key->grp_lock` | Group lock (optional) | Per-key | Application-level lock, can replace key->lock |

### `pj_ioqueue_poll()` Protocol (ioqueue_select.c)

```
LOCK(ioqueue->lock)
    copy rfdset, wfdset, xfdset to local vars
    count = active key count
UNLOCK(ioqueue->lock)

nfds = select(max_fd, &local_rfdset, &local_wfdset, &local_xfdset, timeout)

LOCK(ioqueue->lock)
    for each key in active_list:
        if key in local_wfdset → event[n++] = {key, WRITABLE}
        if key in local_rfdset → event[n++] = {key, READABLE}
        if key in local_xfdset → event[n++] = {key, EXCEPTION}
        increment_counter(key)   // prevent unregister during dispatch
    // limit events to min(count, PJ_IOQUEUE_MAX_EVENTS_IN_SINGLE_POLL)
UNLOCK(ioqueue->lock)

for each event:
    if WRITABLE → ioqueue_dispatch_write_event(key)
    if READABLE → ioqueue_dispatch_read_event(key)
    if EXCEPTION → ioqueue_dispatch_exception_event(key)
    decrement_counter(key)
```

### `ioqueue_dispatch_write_event()` Protocol (ioqueue_common_abs.c)

```
if TRYLOCK(key) fails:
    return    // another thread is processing this key

if write_list is empty:
    UNLOCK(key)
    return

write_op = first entry in write_list

// --- Do the actual send (UNDER KEY LOCK) ---
loop:
    sent = send(key->fd, write_op->buf + write_op->written,
                write_op->size - write_op->written, write_op->flags)
    if sent > 0:
        write_op->written += sent
        if write_op->written < write_op->size:
            continue  // partial write, keep going
        else:
            break     // fully written
    elif EWOULDBLOCK:
        UNLOCK(key)
        return        // will retry on next poll
    else:
        break         // error

// --- Remove completed write_op from list ---
pj_list_erase(write_op)

if write_list is now empty:
    ioqueue_remove_from_set(ioqueue, key, WRITEABLE)

// --- Callback invocation ---
if key->allow_concurrent:
    UNLOCK(key)                         // release BEFORE callback
    (*key->cb.on_write_complete)(key, write_op, write_op->written)
    // NOTE: no re-lock needed
else:
    // DO NOT UNLOCK — hold through callback
    (*key->cb.on_write_complete)(key, write_op, write_op->written)
    UNLOCK(key)                         // release AFTER callback
```

### `pj_ioqueue_send()` Protocol (ioqueue_common_abs.c)

```
// --- Speculative fast path (NO LOCK) ---
if pj_list_empty(&key->write_list):   // safe without lock per comment
    status = pj_sock_send(key->fd, data, &length, flags)
    if status == PJ_SUCCESS:
        return PJ_SUCCESS              // sent immediately, no queuing
    elif status != PJ_STATUS_FROM_OS(EWOULDBLOCK):
        return status                  // real error

// --- Slow path: queue the write ---
write_op = allocate/init write_operation
write_op->buf = data
write_op->size = length
write_op->written = 0
write_op->flags = flags

LOCK(key)
    pj_list_insert_before(&key->write_list, write_op)  // append to FIFO
    ioqueue_add_to_set(ioqueue, key, WRITEABLE)
UNLOCK(key)

return PJ_EPENDING
```

### Key Insight: Why This Is Safe

With `allow_concurrent=false`:
1. **Only one thread** can be inside dispatch for a given key (trylock ensures this)
2. The dispatch thread **holds the key lock through the callback**
3. The callback calls `pj_ioqueue_send()` which re-locks the key — this works because the lock is **recursive**
4. Inside `pj_ioqueue_send`, the fast-path `pj_list_empty` check succeeds (we just removed the completed write_op), so data is sent **immediately** via `send()` syscall
5. The `send()` happens **while still holding the key lock**, so no other thread can interleave a send

With `allow_concurrent=true`:
1. The key lock is released before the callback
2. The callback's `pj_ioqueue_send()` may race with another thread's dispatch
3. This is acceptable when the application handles its own synchronization
4. The stress test tests BOTH modes but the `sequenced` check is what catches ordering violations

---

## 3. Step-by-Step Implementation Plan

### Step 1: Add Recursive Mutex Support

Replace `std::sync::Mutex` with a recursive-capable lock for per-key locking.

**Option A (Recommended): Use `parking_lot::ReentrantMutex`**

```toml
# Cargo.toml
[dependencies]
parking_lot = "0.12"
```

```rust
use parking_lot::ReentrantMutex;
use std::cell::RefCell;

struct IoKeyInner {
    // Replace: inner: Mutex<IoKeyState>
    state: ReentrantMutex<RefCell<IoKeyState>>,
    // ...
}
```

**Option B: Manual recursive lock using thread ID tracking**

```rust
struct RecursiveMutex {
    mutex: Mutex<()>,
    owner: AtomicU64,    // thread ID of current owner, 0 = unowned
    depth: AtomicU32,    // recursion depth
}

impl RecursiveMutex {
    fn lock(&self) -> RecursiveGuard {
        let tid = current_thread_id();
        if self.owner.load(Relaxed) == tid {
            self.depth.fetch_add(1, Relaxed);
        } else {
            let guard = self.mutex.lock().unwrap();
            self.owner.store(tid, Relaxed);
            self.depth.store(1, Relaxed);
            // store guard
        }
        RecursiveGuard { mutex: self }
    }

    fn try_lock(&self) -> Option<RecursiveGuard> {
        let tid = current_thread_id();
        if self.owner.load(Relaxed) == tid {
            self.depth.fetch_add(1, Relaxed);
            Some(RecursiveGuard { mutex: self })
        } else {
            match self.mutex.try_lock() {
                Ok(guard) => {
                    self.owner.store(tid, Relaxed);
                    self.depth.store(1, Relaxed);
                    Some(RecursiveGuard { mutex: self })
                }
                Err(_) => None,
            }
        }
    }
}
```

**Recommendation: Option B** — avoids external dependency and gives explicit control. However, if `parking_lot` is already a dependency, use Option A.

### Step 2: Add `allow_concurrent` Per-Key Flag

```rust
struct IoKeyState {
    // existing fields...
    allow_concurrent: bool,   // NEW: matches pjproject's allow_concurrent
    // REMOVE: processing: bool  // no longer needed for allow_concurrent=false
}
```

Expose via the C API:
```rust
#[no_mangle]
pub extern "C" fn pj_ioqueue_set_concurrency(key: *mut pj_ioqueue_key_t, allow: pj_bool_t) -> pj_status_t {
    // lock key, set allow_concurrent = (allow != 0)
}
```

### Step 3: Restructure `ioqueue_dispatch_write_event`

This is the critical change. Current code (simplified):

```rust
// CURRENT (BROKEN):
fn dispatch_write(key: &IoKey) {
    let mut state = key.state.lock();
    if state.processing { return; }
    state.processing = true;

    // do send...
    let write_op = state.pending_writes.pop_front();
    let cb = state.write_callback;

    drop(state);  // ← RELEASES LOCK
    cb(key, &write_op, bytes_sent);  // callback runs WITHOUT lock

    let mut state = key.state.lock();
    state.processing = false;
}
```

New code:
```rust
// FIXED:
fn dispatch_write(key: &IoKey) {
    // Use trylock — if another thread has this key, skip
    let guard = match key.state.try_lock() {
        Some(g) => g,
        None => return,
    };
    let state = guard.borrow_mut();

    if state.pending_writes.is_empty() {
        return; // guard dropped automatically
    }

    let write_op = &state.pending_writes[0]; // peek, don't remove yet

    // Do send() UNDER LOCK
    let result = send(key.fd, &write_op.buf[write_op.written..], write_op.flags);

    match result {
        Ok(sent) => {
            write_op.written += sent;
            if write_op.written < write_op.size {
                return; // partial write, will retry on next poll
            }
            // Fully written — remove from queue
            let completed_op = state.pending_writes.pop_front().unwrap();

            if state.pending_writes.is_empty() {
                ioqueue_remove_from_set(key, WRITABLE);
            }

            let cb = state.on_write_complete;

            if state.allow_concurrent {
                // Release lock BEFORE callback
                drop(state);
                drop(guard);
                cb(key, &completed_op, completed_op.written);
            } else {
                // Hold lock THROUGH callback (recursive mutex allows this)
                drop(state); // drop RefCell borrow, but NOT the mutex guard
                cb(key, &completed_op, completed_op.written);
                // Mutex guard (guard) released when it drops here
            }
        }
        Err(EWOULDBLOCK) => {
            return; // will retry
        }
        Err(e) => {
            // error handling...
        }
    }
}
```

**CRITICAL DETAIL for RefCell + ReentrantMutex:** When `allow_concurrent=false`, we must:
1. Drop the `RefCell::borrow_mut()` before the callback (so the callback can borrow_mut again)
2. Keep the `ReentrantMutex` guard alive (so the mutex stays locked)
3. The callback's `pj_ioqueue_send()` will re-enter the `ReentrantMutex` (recursive) and do its own `borrow_mut()`

### Step 4: Restructure `ioqueue_send_impl` (Fast Path)

Match pjproject's speculative fast-path:

```rust
fn ioqueue_send_impl(key: &IoKey, data: &[u8], flags: u32) -> pj_status_t {
    // Acquire recursive lock (works from callback because recursive)
    let guard = key.state.lock();

    {
        let state = guard.borrow();
        if !state.pending_writes.is_empty() {
            // Must queue behind existing writes
            drop(state);
            let mut state = guard.borrow_mut();
            let write_op = WriteOperation { buf: data.to_vec(), size: data.len(), written: 0, flags };
            state.pending_writes.push_back(write_op);
            return PJ_EPENDING;
        }
    }

    // Queue empty — try immediate send (still under lock for ordering)
    match pj_sock_send(key.fd, data, flags) {
        Ok(sent) if sent == data.len() => PJ_SUCCESS,
        Ok(sent) => {
            // Partial send — queue remainder
            let mut state = guard.borrow_mut();
            let write_op = WriteOperation {
                buf: data[sent..].to_vec(),
                size: data.len() - sent,
                written: 0,
                flags,
            };
            state.pending_writes.push_back(write_op);
            ioqueue_add_to_set(key, WRITABLE);
            PJ_EPENDING
        }
        Err(EWOULDBLOCK) => {
            let mut state = guard.borrow_mut();
            let write_op = WriteOperation { buf: data.to_vec(), size: data.len(), written: 0, flags };
            state.pending_writes.push_back(write_op);
            ioqueue_add_to_set(key, WRITABLE);
            PJ_EPENDING
        }
        Err(e) => pj_status_from_os(e),
    }
}
```

### Step 5: Restructure `ioqueue_dispatch_read_event`

Same pattern as write dispatch:

```rust
fn dispatch_read(key: &IoKey) {
    let guard = match key.state.try_lock() {
        Some(g) => g,
        None => return,
    };

    // ... do recv() under lock, extract callback info ...

    let cb = { guard.borrow().on_read_complete };

    if guard.borrow().allow_concurrent {
        drop(guard);
        cb(key, data, bytes_read);
    } else {
        // Hold guard through callback
        cb(key, data, bytes_read);
        // guard dropped after callback
    }
}
```

### Step 6: Fix `pj_ioqueue_poll` Event Collection

Match pjproject's pattern of collecting events under the ioqueue lock, then dispatching without it:

```rust
fn ioqueue_poll_impl(ioqueue: &IoQueue, timeout: Duration) -> i32 {
    // 1. Copy fd_sets under ioqueue lock
    let (rset, wset, xset) = {
        let state = ioqueue.state.lock();
        (state.read_set.clone(), state.write_set.clone(), state.except_set.clone())
    };

    // 2. select() with NO locks held
    let nfds = select(&rset, &wset, &xset, timeout);
    if nfds <= 0 { return nfds; }

    // 3. Collect events under ioqueue lock
    let events: Vec<(Arc<IoKey>, EventType)> = {
        let state = ioqueue.state.lock();
        let mut events = Vec::new();
        for key in &state.active_keys {
            if wset.contains(key.fd) {
                events.push((key.clone(), EventType::Write));
            }
            if rset.contains(key.fd) {
                events.push((key.clone(), EventType::Read));
            }
        }
        events
    };

    // 4. Dispatch events WITHOUT ioqueue lock
    let mut count = 0;
    for (key, event_type) in &events {
        match event_type {
            EventType::Write => { dispatch_write(key); count += 1; }
            EventType::Read => { dispatch_read(key); count += 1; }
        }
    }

    count
}
```

### Step 7: Implement `pj_ioqueue_set_concurrency` and `pj_ioqueue_set_default_concurrency`

```rust
#[no_mangle]
pub extern "C" fn pj_ioqueue_set_default_concurrency(
    ioqueue: *mut pj_ioqueue_t,
    allow: pj_bool_t
) -> pj_status_t {
    let ioqueue = unsafe { &*ioqueue };
    let mut state = ioqueue.inner.lock();
    state.default_concurrency = allow != 0;
    PJ_SUCCESS
}

#[no_mangle]
pub extern "C" fn pj_ioqueue_set_concurrency(
    key: *mut pj_ioqueue_key_t,
    allow: pj_bool_t
) -> pj_status_t {
    let key = unsafe { &*key };
    let guard = key.state.lock();
    let mut state = guard.borrow_mut();
    state.allow_concurrent = allow != 0;
    PJ_SUCCESS
}
```

---

## 4. Lock Ordering Rules

pjproject enforces this lock ordering to prevent deadlock:

```
grp_lock (application) → key->lock (per-key) → ioqueue->lock (global)
```

Specific rules:
1. **Never hold ioqueue lock when acquiring key lock** (the dispatch loop releases ioqueue lock before calling dispatch functions)
2. **Never hold key lock when acquiring ioqueue lock** — EXCEPT in `ioqueue_add_to_set` / `ioqueue_remove_from_set` which are called from within dispatch while holding the key lock. In pjproject, these functions acquire the ioqueue lock briefly. **This is the one exception** and it's safe because:
   - Key lock → ioqueue lock is always the order
   - Ioqueue lock → key lock never happens (trylock is used in poll after releasing ioqueue lock)
3. **grp_lock can be held when entering key lock** — the callback may hold grp_lock and call `pj_ioqueue_send()` which locks the key. This is fine because key lock is never held when acquiring grp_lock.

### Our Implementation Must Follow:

```
Application lock (grp_lock) → key.state (recursive) → ioqueue.state
                                    ↑                         ↑
                              never reversed           never reversed
```

In `ioqueue_add_to_set` / `ioqueue_remove_from_set`:
```rust
// Called while key lock is held (from dispatch or send)
fn ioqueue_add_to_set(ioqueue: &IoQueue, key_fd: RawFd, set: SetType) {
    let mut state = ioqueue.state.lock(); // ioqueue lock acquired AFTER key lock — OK
    match set {
        SetType::Write => state.write_set.insert(key_fd),
        SetType::Read => state.read_set.insert(key_fd),
    }
}
```

---

## 5. Known Pitfalls

### Pitfall 1: Recursive Mutex is Required
`std::sync::Mutex` will deadlock if the same thread tries to lock it twice. With `allow_concurrent=false`, the dispatch holds the key lock and the callback calls `pj_ioqueue_send()` which locks the key again. **A recursive mutex is mandatory.**

### Pitfall 2: `RefCell` Inside `ReentrantMutex`
`parking_lot::ReentrantMutex<T>` gives you `&T`, not `&mut T` (because multiple guards can exist for the same thread). To get mutable access, wrap in `RefCell`: `ReentrantMutex<RefCell<IoKeyState>>`. This means runtime borrow checking — a double `borrow_mut()` on the same `RefCell` will panic. Be careful not to hold a `borrow_mut()` when recursing into a function that also borrows.

**Solution:** Structure code so that mutable borrows are short-lived:
```rust
let guard = key.state.lock(); // ReentrantMutex guard
{
    let mut state = guard.borrow_mut();
    // read/write state
    // extract what we need (callback fn ptr, fd, etc.)
} // borrow_mut dropped here
// Now safe to call callback which may recurse and borrow_mut again
```

### Pitfall 3: Callback Must Not Deadlock on Ioqueue Lock
The callback might call `pj_ioqueue_send()` → `ioqueue_add_to_set()` → acquires ioqueue lock. This is fine as long as the ioqueue lock is never held when trying to get a key lock. Our poll function must release the ioqueue lock before dispatching.

### Pitfall 4: The `processing` Flag Can Be Removed
For `allow_concurrent=true`, pjproject doesn't use a processing flag — it uses `trylock`. The trylock in dispatch already prevents two threads from processing the same key simultaneously. The `processing` flag is our invention and can be removed entirely — just use trylock.

### Pitfall 5: Unregistration During Dispatch
pjproject uses `increment_counter` / `decrement_counter` on keys during event collection to prevent a key from being fully destroyed while dispatch is in progress. Our `Arc<IoKey>` reference counting provides similar safety, but we should ensure:
- `pj_ioqueue_unregister()` removes the key from active_list under the ioqueue lock
- The dispatch loop holds an `Arc` clone, so the key stays alive
- The actual cleanup runs when the last `Arc` is dropped

### Pitfall 6: Partial Writes on Stream Sockets
pjproject's dispatch loop continues sending in a loop until the entire buffer is written or EWOULDBLOCK. Our implementation must do the same — don't just send once and declare success. The `write_op.written` field tracks progress.

### Pitfall 7: `select()` Spurious Readiness
After `select()` reports a socket as writable, the actual `send()` might still return EWOULDBLOCK (especially under heavy contention). Handle this gracefully by just returning from dispatch and waiting for the next poll.

### Pitfall 8: Thread Safety of `pj_list_empty` Check
pjproject's `pj_ioqueue_send()` reads `key->write_list.next == &key->write_list` without holding the lock. This is noted in a comment as intentionally racy — the worst case is falling through to the slow path. In Rust, we can't safely do an unsynchronized read. Options:
- Use `AtomicBool has_pending_writes` updated under the key lock
- Or just take the recursive lock (it's fast when uncontended, and with `allow_concurrent=false` it's a recursive re-entry which is essentially free)

### Pitfall 9: Drop Order Matters
When dispatching with `allow_concurrent=true`, we drop the lock guard before the callback. Ensure the `WriteOperation` and callback function pointer are extracted before dropping the guard, as the state won't be accessible after.

---

## 6. Testing Strategy

### Step 1: Unit Test — Recursive Mutex
Verify the recursive mutex works: lock, lock again from same thread, unlock, unlock.

### Step 2: Unit Test — Allow Concurrent Semantics
Test that with `allow_concurrent=false`:
- Callback runs while key lock is held
- `pj_ioqueue_send()` from callback doesn't deadlock
- A second thread's trylock during callback fails

### Step 3: Integration Test — TCP Sequencing
Port the exact stress test logic:
- 16 threads polling
- TCP socket pairs
- Sequential integer verification
- Both `allow_concurrent=true` and `allow_concurrent=false`

### Step 4: Run Original Stress Test
Build against the actual pjproject test harness and verify rc=0.

---

## 7. Implementation Order

1. **Implement recursive mutex** (or add `parking_lot` dependency)
2. **Add `allow_concurrent` field** to `IoKeyState`
3. **Restructure `dispatch_write`** to hold lock through callback when `allow_concurrent=false`
4. **Restructure `dispatch_read`** same pattern
5. **Fix `ioqueue_send_impl`** fast-path to work with recursive locking
6. **Fix `ioqueue_recv_impl`** similarly
7. **Expose `pj_ioqueue_set_concurrency`** C API
8. **Fix poll loop** — ensure ioqueue lock is released before dispatch
9. **Remove `processing` flag** — replaced by trylock semantics
10. **Run stress test** — iterate until rc=0

---

## 8. Summary of Changes

| Component | Current | Required |
|-----------|---------|----------|
| Per-key lock | `std::sync::Mutex` (non-recursive) | Recursive mutex |
| Concurrency control | `processing` flag | `allow_concurrent` + trylock |
| Lock during callback | Released before callback | Held through callback when `allow_concurrent=false` |
| Send fast-path | Under non-recursive lock | Under recursive lock or lock-free hint |
| Lock ordering | Not documented | key lock → ioqueue lock (strict) |
| Partial write tracking | Unknown | `written` field in WriteOperation |

**Expected outcome:** With these changes, the key lock is held during the entire dispatch→callback→send cycle, ensuring TCP byte ordering is maintained even with 16 concurrent poll threads.
