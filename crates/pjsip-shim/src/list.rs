//! pj_list -- doubly-linked list operations on C structs with prev/next pointers.
//!
//! pjproject uses intrusive doubly-linked lists everywhere.  The list node
//! layout is: `{ prev: *mut T, next: *mut T, ... }`.  All operations work
//! on raw pointers to the node header.

/// Generic list node.  In pjproject every list node starts with prev/next
/// pointers.  We operate on `*mut pj_list_node` which is really a pointer
/// to the first two pointer-sized fields of any struct.
#[repr(C)]
pub struct pj_list_node {
    pub prev: *mut pj_list_node,
    pub next: *mut pj_list_node,
}

// ---------------------------------------------------------------------------
// pj_list_init
// ---------------------------------------------------------------------------

/// Initialize a list head (sentinel) so it points to itself.
#[no_mangle]
pub unsafe extern "C" fn pj_list_init(node: *mut pj_list_node) {
    if node.is_null() {
        return;
    }
    (*node).prev = node;
    (*node).next = node;
}

// ---------------------------------------------------------------------------
// pj_list_insert_before
// ---------------------------------------------------------------------------

/// Insert `node` before `pos`.
#[no_mangle]
pub unsafe extern "C" fn pj_list_insert_before(
    pos: *mut pj_list_node,
    node: *mut pj_list_node,
) {
    if pos.is_null() || node.is_null() {
        return;
    }
    let prev = (*pos).prev;
    (*node).prev = prev;
    (*node).next = pos;
    (*prev).next = node;
    (*pos).prev = node;
}

// ---------------------------------------------------------------------------
// pj_list_insert_after
// ---------------------------------------------------------------------------

/// Insert `node` after `pos`.
#[no_mangle]
pub unsafe extern "C" fn pj_list_insert_after(
    pos: *mut pj_list_node,
    node: *mut pj_list_node,
) {
    if pos.is_null() || node.is_null() {
        return;
    }
    let next = (*pos).next;
    (*node).prev = pos;
    (*node).next = next;
    (*next).prev = node;
    (*pos).next = node;
}

// ---------------------------------------------------------------------------
// pj_list_insert_nodes_before
// ---------------------------------------------------------------------------

/// Insert a sub-list (lst) before `pos`.  `lst` is a list head whose nodes
/// are spliced into the list before `pos`.
#[no_mangle]
pub unsafe extern "C" fn pj_list_insert_nodes_before(
    pos: *mut pj_list_node,
    lst: *mut pj_list_node,
) {
    if pos.is_null() || lst.is_null() {
        return;
    }
    // If lst is empty (points to itself), nothing to do
    if (*lst).next == lst {
        return;
    }
    let first = (*lst).next;
    let last = (*lst).prev;
    let prev = (*pos).prev;

    // Splice in
    (*prev).next = first;
    (*first).prev = prev;
    (*last).next = pos;
    (*pos).prev = last;

    // Reset lst to empty
    (*lst).next = lst;
    (*lst).prev = lst;
}

// ---------------------------------------------------------------------------
// pj_list_erase
// ---------------------------------------------------------------------------

/// Remove a node from its list.
#[no_mangle]
pub unsafe extern "C" fn pj_list_erase(node: *mut pj_list_node) {
    if node.is_null() {
        return;
    }
    let prev = (*node).prev;
    let next = (*node).next;
    (*prev).next = next;
    (*next).prev = prev;
    // Detach
    (*node).prev = node;
    (*node).next = node;
}

// ---------------------------------------------------------------------------
// pj_list_find_node
// ---------------------------------------------------------------------------

/// Find a node in the list.  Returns the node pointer if found, or null.
#[no_mangle]
pub unsafe extern "C" fn pj_list_find_node(
    list: *mut pj_list_node,
    node: *mut pj_list_node,
) -> *mut pj_list_node {
    if list.is_null() || node.is_null() {
        return std::ptr::null_mut();
    }
    let mut cur = (*list).next;
    while cur != list {
        if cur == node {
            return cur;
        }
        cur = (*cur).next;
    }
    std::ptr::null_mut()
}

// ---------------------------------------------------------------------------
// pj_list_search
// ---------------------------------------------------------------------------

/// Search a list using a comparison function.
/// The comparison function receives (node, value) and should return 0 on match.
#[no_mangle]
pub unsafe extern "C" fn pj_list_search(
    list: *mut pj_list_node,
    value: *mut libc::c_void,
    comp: Option<unsafe extern "C" fn(*const libc::c_void, *const libc::c_void) -> i32>,
) -> *mut pj_list_node {
    if list.is_null() {
        return std::ptr::null_mut();
    }
    let comp = match comp {
        Some(f) => f,
        None => return std::ptr::null_mut(),
    };
    let mut cur = (*list).next;
    while cur != list {
        if comp(cur as *const _, value as *const _) == 0 {
            return cur;
        }
        cur = (*cur).next;
    }
    std::ptr::null_mut()
}

// ---------------------------------------------------------------------------
// pj_list_size
// ---------------------------------------------------------------------------

/// Return the number of nodes in the list (excluding the sentinel).
#[no_mangle]
pub unsafe extern "C" fn pj_list_size(list: *const pj_list_node) -> usize {
    if list.is_null() {
        return 0;
    }
    let mut count = 0usize;
    let mut cur = (*list).next;
    while cur != list as *mut _ {
        count += 1;
        cur = (*cur).next;
    }
    count
}

// ---------------------------------------------------------------------------
// pj_list_merge_first
// ---------------------------------------------------------------------------

/// Merge list2 into list1, inserting all elements of list2 at the beginning
/// of list1.
#[no_mangle]
pub unsafe extern "C" fn pj_list_merge_first(
    list1: *mut pj_list_node,
    list2: *mut pj_list_node,
) {
    if list1.is_null() || list2.is_null() {
        return;
    }
    if (*list2).next == list2 {
        return; // list2 is empty
    }
    let first2 = (*list2).next;
    let last2 = (*list2).prev;
    let first1 = (*list1).next;

    // Splice list2 at the beginning of list1
    (*list1).next = first2;
    (*first2).prev = list1;
    (*last2).next = first1;
    (*first1).prev = last2;

    // Reset list2 to empty
    (*list2).next = list2;
    (*list2).prev = list2;
}
