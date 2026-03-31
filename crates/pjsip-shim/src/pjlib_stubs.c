/*
 * pjlib_stubs.c -- stub definitions for symbols required by the real pjlib
 * C source files (os_core_unix.c, lock.c, os_timestamp_posix.c) that we
 * compile into our shared library but don't want to pull in the full
 * pjlib source tree for.
 */

#include <pj/types.h>
#include <pj/config.h>

/* ---------- PJ_NO_MEMORY_EXCEPTION ---------- */
/* Defined in pool.c, referenced by os_core_unix.c's pj_init(). */
int PJ_NO_MEMORY_EXCEPTION = 0;

int pj_NO_MEMORY_EXCEPTION(void)
{
    return PJ_NO_MEMORY_EXCEPTION;
}

/* ---------- PJ_VERSION ---------- */
/* Defined in config.c; referenced by os_core_unix.c's pj_init() log message. */
const char *PJ_VERSION = "2.16";

const char *pj_get_version(void)
{
    return PJ_VERSION;
}

/* ---------- pj_log_init ---------- */
/* Defined in log.c; called by pj_init() to set up thread-local log state.
 * Our logging is handled by log_wrapper.c -- just return success. */
pj_status_t pj_log_init(void)
{
    return PJ_SUCCESS;
}

/* ---------- pj_errno_clear_handlers ---------- */
/* Defined in errno.c; called by pj_shutdown(). No-op for us. */
void pj_errno_clear_handlers(void)
{
}
