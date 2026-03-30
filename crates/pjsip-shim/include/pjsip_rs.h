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

/* ------------------------------------------------------------------ */
/* pj_str_t                                                            */
/* ------------------------------------------------------------------ */

typedef struct pj_str_t {
    char     *ptr;
    ptrdiff_t slen;
} pj_str_t;

/* ------------------------------------------------------------------ */
/* Opaque types                                                        */
/* ------------------------------------------------------------------ */

typedef struct pj_pool_t        pj_pool_t;
typedef struct pj_pool_factory {
    char _opaque[0];
} pj_pool_factory;
typedef struct pjsip_uri        pjsip_uri;
typedef struct pjsip_sip_uri    pjsip_sip_uri;
typedef struct pjsip_msg        pjsip_msg;
typedef struct pjsip_hdr        pjsip_hdr;
typedef struct pjsip_endpoint   pjsip_endpoint;

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

pjsip_msg* pjsip_parse_msg(pj_pool_t *pool, char *buf, size_t size,
                            unsigned options);
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
