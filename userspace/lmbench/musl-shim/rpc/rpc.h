/* Minimal stub: lmbench includes <rpc/rpc.h> unconditionally but the
 * benchmarked paths (sockets, memory, syscalls) never call into it.
 * musl has no sunrpc. The two extern prototypes in bench.h reference
 * sunrpc types that are never used by the benchmarked binaries. */
#ifndef LMBENCH_RPC_STUB_H
#define LMBENCH_RPC_STUB_H
#include <sys/types.h>
typedef unsigned long u_long;
typedef struct svcxprt_stub SVCXPRT;
typedef struct client_stub CLIENT;
/* portmap stubs: the RPC benchmarks are out of scope; lib_tcp/lib_udp
 * reference these but their socket benchmarks never reach them. */
/* Variadic: the call sites pass varying sub-argument counts. */
static inline int pmap_unset(unsigned long a, ...) { (void)a; return 0; }
static inline int pmap_set(unsigned long a, ...) { (void)a; return 0; }
static inline unsigned short pmap_getport(void *a, ...) { (void)a; return 0; }
#endif
