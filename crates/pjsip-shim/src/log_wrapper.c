#include <stdio.h>
#include <stdarg.h>
#include <stdlib.h>
#include <setjmp.h>

/* Our Rust function that takes the already-formatted string. */
extern void pj_log_write(int level, const char *sender, const char *msg);

/* ------------------------------------------------------------------ */
/* Exception handling (setjmp/longjmp)                                 */
/* Matches pjproject's pj/except.h layout:                             */
/*   struct pj_exception_state_t { jmp_buf buf; prev; }               */
/* The exception ID is conveyed via the longjmp return value only      */
/* (PJ_GET_EXCEPTION() reads setjmp's return, not a struct field).    */
/* ------------------------------------------------------------------ */

struct exception_state {
    jmp_buf buf;
    struct exception_state *prev;
};

static __thread struct exception_state *exception_stack = NULL;

void pj_push_exception_handler_(void *rec) {
    struct exception_state *st = (struct exception_state *)rec;
    st->prev = exception_stack;
    exception_stack = st;
}

void pj_pop_exception_handler_(void *rec) {
    struct exception_state *st = (struct exception_state *)rec;
    exception_stack = st->prev;
}

void pj_throw_exception_(int id) {
    struct exception_state *st = exception_stack;
    if (st) {
        /* Pop the handler before longjmp so that a re-throw from the
           catch block propagates to the outer handler, not this one. */
        pj_pop_exception_handler_(st);
        longjmp(st->buf, id);
    } else {
        fprintf(stderr, "pj_throw_exception_(%d): no handler, aborting\n", id);
        abort();
    }
}

/* Non-underscore aliases (some pjproject code uses these) */
void pj_push_exception_handler(void *rec) {
    pj_push_exception_handler_(rec);
}

void pj_pop_exception_handler(void *rec) {
    pj_pop_exception_handler_(rec);
}

void pj_throw_exception(int id) {
    pj_throw_exception_(id);
}

/* ------------------------------------------------------------------ */
/* pj_log_1 .. pj_log_5                                               */
/* C signature: void pj_log_N(const char *sender, const char *fmt, ...); */
/* ------------------------------------------------------------------ */

void pj_log_1(const char *sender, const char *fmt, ...) {
    char buf[4096];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof(buf), fmt, ap);
    va_end(ap);
    pj_log_write(1, sender, buf);
}

void pj_log_2(const char *sender, const char *fmt, ...) {
    char buf[4096];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof(buf), fmt, ap);
    va_end(ap);
    pj_log_write(2, sender, buf);
}

void pj_log_3(const char *sender, const char *fmt, ...) {
    char buf[4096];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof(buf), fmt, ap);
    va_end(ap);
    pj_log_write(3, sender, buf);
}

void pj_log_4(const char *sender, const char *fmt, ...) {
    char buf[4096];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof(buf), fmt, ap);
    va_end(ap);
    pj_log_write(4, sender, buf);
}

void pj_log_5(const char *sender, const char *fmt, ...) {
    char buf[4096];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof(buf), fmt, ap);
    va_end(ap);
    pj_log_write(5, sender, buf);
}

/* ------------------------------------------------------------------ */
/* pj_perror_1 .. pj_perror_5                                          */
/* C signature: void pj_perror_N(const char *sender, const char *title,*/
/*                               pj_status_t status, const char *fmt, ...); */
/* ------------------------------------------------------------------ */

void pj_perror_1(const char *sender, const char *title __attribute__((unused)),
                 int status __attribute__((unused)), const char *fmt, ...) {
    char buf[4096];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof(buf), fmt, ap);
    va_end(ap);
    pj_log_write(1, sender, buf);
}

void pj_perror_2(const char *sender, const char *title __attribute__((unused)),
                 int status __attribute__((unused)), const char *fmt, ...) {
    char buf[4096];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof(buf), fmt, ap);
    va_end(ap);
    pj_log_write(2, sender, buf);
}

void pj_perror_3(const char *sender, const char *title __attribute__((unused)),
                 int status __attribute__((unused)), const char *fmt, ...) {
    char buf[4096];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof(buf), fmt, ap);
    va_end(ap);
    pj_log_write(3, sender, buf);
}

void pj_perror_4(const char *sender, const char *title __attribute__((unused)),
                 int status __attribute__((unused)), const char *fmt, ...) {
    char buf[4096];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof(buf), fmt, ap);
    va_end(ap);
    pj_log_write(4, sender, buf);
}

void pj_perror_5(const char *sender, const char *title __attribute__((unused)),
                 int status __attribute__((unused)), const char *fmt, ...) {
    char buf[4096];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof(buf), fmt, ap);
    va_end(ap);
    pj_log_write(5, sender, buf);
}

/* ------------------------------------------------------------------ */
/* pj_perror (generic, level-parameterised)                            */
/* C signature: void pj_perror(int level, const char *sender,          */
/*                             pj_status_t status, const char *fmt, ...); */
/* ------------------------------------------------------------------ */

void pj_perror(int level, const char *sender,
               int status __attribute__((unused)), const char *fmt, ...) {
    char buf[4096];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof(buf), fmt, ap);
    va_end(ap);
    pj_log_write(level, sender, buf);
}
