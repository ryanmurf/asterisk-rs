/*
 * pjsip_rs.h -- C header for the Rust pjsip shim library.
 *
 * This header declares the public API of libpjsip_rs, a drop-in
 * replacement for pjproject's libpj + libpjsip.  Link against
 * libpjsip_rs.dylib (macOS) or libpjsip_rs.so (Linux) and include
 * this header instead of the original pjsip headers.
 */
#ifndef PJSIP_RS_H
#define PJSIP_RS_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ------------------------------------------------------------------ */
/* Status codes                                                        */
/* ------------------------------------------------------------------ */

typedef int32_t pj_status_t;
typedef int32_t pj_bool_t;
typedef size_t pj_size_t;
typedef ptrdiff_t pj_ssize_t;
typedef uint64_t pj_timestamp;
typedef uint64_t pj_highprec_t;
typedef uint64_t pj_time_val;

#define PJ_SUCCESS      0
#define PJ_EINVAL       70014
#define PJ_ENOMEM       70015
#define PJ_ENOTFOUND    70018
#define PJ_ETOOMANY     70027
#define PJ_EEOF         70028
#define PJ_EBUSY        70029
#define PJ_EINVALIDOP   70030

#define PJ_TRUE         1
#define PJ_FALSE        0

/* PJSIP status codes */
#define PJSIP_EPARTIALMSG   171061

/* ------------------------------------------------------------------ */
/* Logging macros                                                      */
/* ------------------------------------------------------------------ */

#define PJ_LOG(level, arg) do { \
    if ((level) <= pj_log_get_level()) { \
        /* Simplified logging - just print to stdout for now */ \
        printf arg; \
        printf("\n"); \
    } \
} while (0)

#include <stdio.h>

/* ------------------------------------------------------------------ */
/* pj_str_t                                                            */
/* ------------------------------------------------------------------ */

typedef struct pj_str_t {
    char     *ptr;
    ptrdiff_t slen;
} pj_str_t;

/* ------------------------------------------------------------------ */
/* List structure - simplified for compatibility                      */
/* ------------------------------------------------------------------ */

typedef struct pj_list {
    void *prev;
    void *next;
} pj_list;

/* ------------------------------------------------------------------ */
/* Complete struct definitions                                         */
/* ------------------------------------------------------------------ */

typedef struct pj_pool_t        pj_pool_t;
typedef struct pj_pool_factory {
    char _opaque[0];
} pj_pool_factory;

typedef struct pjsip_endpoint   pjsip_endpoint;

/* Message type constants */
#define PJSIP_REQUEST_MSG  0
#define PJSIP_RESPONSE_MSG 1

/* Header types */
#define PJSIP_H_VIA             1
#define PJSIP_H_FROM            2
#define PJSIP_H_TO              3
#define PJSIP_H_CALL_ID         4
#define PJSIP_H_CSEQ            5
#define PJSIP_H_CONTACT         6
#define PJSIP_H_CONTENT_TYPE    7
#define PJSIP_H_CONTENT_LENGTH  8
#define PJSIP_H_OTHER           63

typedef struct pjsip_method {
    int id;
    pj_str_t name;
} pjsip_method;

typedef struct pjsip_uri {
    const void *vptr;
} pjsip_uri;

typedef struct pjsip_sip_uri {
    const void *vptr;
    pj_str_t scheme;
    pj_str_t user;
    pj_str_t passwd;
    pj_str_t host;
    int port;
    pj_str_t transport_param;
    pj_str_t user_param;
    pj_str_t method_param;
    int ttl_param;
    int lr_param;
    pj_str_t maddr_param;
} pjsip_sip_uri;

typedef struct pjsip_hdr {
    struct pjsip_hdr *prev;
    struct pjsip_hdr *next;
    int htype;
    pj_str_t name;
    pj_str_t sname;
} pjsip_hdr;

typedef struct pjsip_name_addr {
    struct pjsip_hdr *prev;
    struct pjsip_hdr *next;
    int htype;
    pj_str_t name;
    pj_str_t sname;
    pj_str_t display;
    pjsip_uri *uri;
} pjsip_name_addr;

typedef struct pjsip_request_line {
    pjsip_method method;
    pjsip_uri *uri;
} pjsip_request_line;

typedef struct pjsip_status_line {
    int code;
    pj_str_t reason;
} pjsip_status_line;

typedef union pjsip_msg_line {
    pjsip_request_line req;
    pjsip_status_line status;
} pjsip_msg_line;

typedef struct pjsip_media_type {
    pj_str_t type;
    pj_str_t subtype;
} pjsip_media_type;

typedef struct pjsip_msg_body {
    pjsip_media_type content_type;
    void *data;
    unsigned int len;
} pjsip_msg_body;

typedef struct pjsip_msg {
    int type;
    pjsip_msg_line line;
    pjsip_hdr hdr;
    pjsip_msg_body *body;
} pjsip_msg;

/* Parser error report list */
typedef struct pjsip_parser_err_report {
    pj_list list;
    int line;
    int col;
    pj_str_t except_code;
    char *hname;
} pjsip_parser_err_report;

typedef struct pj_caching_pool {
    pj_pool_factory factory;
    char _pad[256];
} pj_caching_pool;

/* ------------------------------------------------------------------ */
/* Sockaddr types                                                      */
/* ------------------------------------------------------------------ */

typedef struct pj_in_addr {
    uint32_t s_addr;
} pj_in_addr;

typedef struct pj_sockaddr_in {
    uint16_t   sin_family;
    uint16_t   sin_port;
    pj_in_addr sin_addr;
    uint8_t    sin_zero[8];
} pj_sockaddr_in;

typedef struct pj_in6_addr {
    uint8_t s6_addr[16];
} pj_in6_addr;

typedef struct pj_sockaddr_in6 {
    uint16_t    sin6_family;
    uint16_t    sin6_port;
    uint32_t    sin6_flowinfo;
    pj_in6_addr sin6_addr;
    uint32_t    sin6_scope_id;
} pj_sockaddr_in6;

typedef union pj_sockaddr {
    pj_sockaddr_in  addr;
    pj_sockaddr_in6 ipv6;
} pj_sockaddr;

#define PJ_AF_INET   2
#if defined(__APPLE__)
#define PJ_AF_INET6  30
#else
#define PJ_AF_INET6  10
#endif

/* ------------------------------------------------------------------ */
/* String / Utility Functions                                         */
/* ------------------------------------------------------------------ */

size_t pj_ansi_strlen(const char *str);
int pj_ansi_snprintf(char *buf, size_t size, const char *fmt, ...);
char* pj_ansi_strxcpy(char *dst, const char *src, size_t size);
void pj_bzero(void *ptr, size_t size);

/* ------------------------------------------------------------------ */
/* Timing Functions                                                   */
/* ------------------------------------------------------------------ */

void pj_get_timestamp(pj_timestamp *ts);
void pj_add_timestamp(pj_timestamp *ts1, const pj_timestamp *ts2);
void pj_sub_timestamp(pj_timestamp *ts1, const pj_timestamp *ts2);
uint64_t pj_elapsed_time(const pj_timestamp *start, const pj_timestamp *stop);
uint64_t pj_elapsed_usec(const pj_timestamp *start, const pj_timestamp *stop);
pj_highprec_t pj_highprec_div(pj_highprec_t val, pj_highprec_t div);
pj_highprec_t pj_highprec_mod(pj_highprec_t val, pj_highprec_t mod);
pj_highprec_t pj_highprec_mul(pj_highprec_t val, pj_highprec_t mul);

/* ------------------------------------------------------------------ */
/* List Functions                                                     */
/* ------------------------------------------------------------------ */

void pj_list_init(pj_list *list);
void pj_list_push_back(pj_list *list, void *node);
size_t pj_list_size(const pj_list *list);

/* ------------------------------------------------------------------ */
/* SIP Message Functions                                              */
/* ------------------------------------------------------------------ */

pj_status_t pjsip_find_msg(const char *buf, pj_size_t len, 
                           pj_bool_t is_datagram, pj_size_t *msg_size);

pjsip_msg* pjsip_parse_msg(pj_pool_t *pool, char *buf, pj_size_t size,
                           pjsip_parser_err_report *err_list);

pj_ssize_t pjsip_msg_print(const pjsip_msg *msg, char *buf, pj_size_t size);

int pjsip_method_cmp(const pjsip_method *m1, const pjsip_method *m2);
void pjsip_method_set(pjsip_method *m, int method_id, const pj_str_t *method_name);

pj_ssize_t pjsip_hdr_print_on(void *hdr, char *buf, pj_size_t size);

pjsip_hdr* pjsip_parse_hdr(pj_pool_t *pool, const pj_str_t *name,
                           char *buf, pj_size_t size, int *parsed_len);

/* ------------------------------------------------------------------ */
/* SIP Header Creation Functions                                      */
/* ------------------------------------------------------------------ */

pjsip_hdr* pjsip_cid_hdr_create(pj_pool_t *pool);
pjsip_hdr* pjsip_clen_hdr_create(pj_pool_t *pool);
pjsip_hdr* pjsip_contact_hdr_create(pj_pool_t *pool);
pjsip_hdr* pjsip_cseq_hdr_create(pj_pool_t *pool, unsigned seq, const pjsip_method *method);
pjsip_hdr* pjsip_ctype_hdr_create(pj_pool_t *pool);
pjsip_hdr* pjsip_from_hdr_create(pj_pool_t *pool);
pjsip_hdr* pjsip_generic_string_hdr_create(pj_pool_t *pool, const pj_str_t *name, const pj_str_t *value);
pjsip_hdr* pjsip_max_fwd_hdr_create(pj_pool_t *pool, int value);
pjsip_name_addr* pjsip_name_addr_create(pj_pool_t *pool);

/* ------------------------------------------------------------------ */
/* URI Functions                                                      */
/* ------------------------------------------------------------------ */

int pjsip_uri_cmp(int context, const pjsip_uri *uri1, const pjsip_uri *uri2);

#define PJSIP_URI_IN_REQ_URI    1

/* ------------------------------------------------------------------ */
/* Misc Functions                                                     */
/* ------------------------------------------------------------------ */

void app_perror(const char *msg, pj_status_t status);

/* ------------------------------------------------------------------ */
/* Init / shutdown                                                     */
/* ------------------------------------------------------------------ */

pj_status_t pj_init(void);
pj_status_t pj_shutdown(void);
pj_status_t pjlib_util_init(void);

/* Logging */
void pj_log_set_level(int level);
int  pj_log_get_level(void);
void pj_log_set_decor(unsigned decor);
void pj_log_set_log_func(void *func);

/* ------------------------------------------------------------------ */
/* Pool                                                                */
/* ------------------------------------------------------------------ */

pj_pool_t* pj_pool_create(void *factory, const char *name,
                           size_t initial, size_t increment, void *cb);
void*      pj_pool_alloc(pj_pool_t *pool, size_t size);
void*      pj_pool_zalloc(pj_pool_t *pool, size_t size);
void*      pj_pool_calloc(pj_pool_t *pool, size_t count, size_t size);
void       pj_pool_release(pj_pool_t *pool);
void       pj_pool_reset(pj_pool_t *pool);
size_t     pj_pool_get_used_size(const pj_pool_t *pool);
size_t     pj_pool_get_capacity(const pj_pool_t *pool);

/* Caching pool */
void pj_caching_pool_init(pj_caching_pool *cp, const void *policy,
                           size_t max_capacity);
void pj_caching_pool_destroy(pj_caching_pool *cp);

/* ------------------------------------------------------------------ */
/* String                                                              */
/* ------------------------------------------------------------------ */

pj_str_t   pj_str(char *s);
ptrdiff_t  pj_strlen(const pj_str_t *s);
char*      pj_strbuf(const pj_str_t *s);

int  pj_strcmp(const pj_str_t *s1, const pj_str_t *s2);
int  pj_strcmp2(const pj_str_t *s1, const char *s2);
int  pj_stricmp(const pj_str_t *s1, const pj_str_t *s2);
int  pj_stricmp2(const pj_str_t *s1, const char *s2);

void        pj_strdup(pj_pool_t *pool, pj_str_t *dst, const pj_str_t *src);
void        pj_strdup2(pj_pool_t *pool, pj_str_t *dst, const char *src);
void        pj_strdup_with_null(pj_pool_t *pool, pj_str_t *dst, const pj_str_t *src);
void        pj_strassign(pj_str_t *dst, const pj_str_t *src);
pj_str_t*   pj_strcpy(pj_str_t *dst, const pj_str_t *src);
pj_str_t*   pj_strcpy2(pj_str_t *dst, const char *src);
pj_str_t*   pj_strset(pj_str_t *s, char *ptr, ptrdiff_t len);
pj_str_t*   pj_strset2(pj_str_t *s, char *src);
pj_str_t*   pj_strset3(pj_str_t *s, char *begin, char *end);

ptrdiff_t    pj_strfind(const pj_str_t *s, const pj_str_t *sub);
const char*  pj_strchr(const pj_str_t *s, int c);
void         pj_strtrim(pj_str_t *s);
long         pj_strtol(const pj_str_t *s);
unsigned long pj_strtoul(const pj_str_t *s);

/* ------------------------------------------------------------------ */
/* URI                                                                 */
/* ------------------------------------------------------------------ */

pjsip_uri* pjsip_parse_uri(pj_pool_t *pool, char *buf, size_t size,
                            unsigned options);

/* ------------------------------------------------------------------ */
/* Message                                                             */
/* ------------------------------------------------------------------ */

pjsip_msg* pjsip_msg_create(pj_pool_t *pool, int type);
pjsip_hdr* pjsip_msg_find_hdr(const pjsip_msg *msg, int type,
                               const pjsip_hdr *start);
pjsip_hdr* pjsip_msg_find_hdr_by_name(const pjsip_msg *msg,
                                       const pj_str_t *name,
                                       const pjsip_hdr *start);
void       pjsip_msg_add_hdr(pjsip_msg *msg, pjsip_hdr *hdr);
pjsip_hdr* pjsip_hdr_clone(pj_pool_t *pool, const pjsip_hdr *hdr);

/* ------------------------------------------------------------------ */
/* Endpoint                                                            */
/* ------------------------------------------------------------------ */

pj_status_t pjsip_endpt_create(void *pf, const char *name,
                                pjsip_endpoint **endpt);
void        pjsip_endpt_destroy(pjsip_endpoint *endpt);
void*       pjsip_endpt_get_pool_factory(pjsip_endpoint *endpt);
pj_pool_t*  pjsip_endpt_create_pool(pjsip_endpoint *endpt,
                                     const char *name,
                                     size_t initial, size_t increment);
void        pjsip_endpt_release_pool(pjsip_endpoint *endpt,
                                     pj_pool_t *pool);

/* ------------------------------------------------------------------ */
/* Sockaddr                                                            */
/* ------------------------------------------------------------------ */

pj_status_t pj_sockaddr_parse(int af, unsigned options,
                               const pj_str_t *addr_str,
                               pj_sockaddr *addr);
char*       pj_sockaddr_print(const pj_sockaddr *addr, char *buf,
                               int size, unsigned with_port);
uint16_t    pj_sockaddr_get_port(const pj_sockaddr *addr);
void        pj_sockaddr_set_port(pj_sockaddr *addr, uint16_t port);
pj_status_t pj_sockaddr_init(int af, pj_sockaddr *addr,
                              const pj_str_t *host, uint16_t port);

#ifdef __cplusplus
}
#endif

#endif /* PJSIP_RS_H */
