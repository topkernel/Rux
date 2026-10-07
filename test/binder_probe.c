/* binder_probe.c — OpenHarmony port Spike S1: /dev/binder closed-loop probe.
 *
 * Two processes over the misc char device /dev/binder:
 *
 *   A (client, parent)                 B (server, child)
 *   -----------------                  -----------------
 *   open + mmap 1MiB                   open + mmap 1MiB
 *   BINDER_VERSION -> 8                BINDER_VERSION -> 8
 *   SET_MAX_THREADS(4)                 SET_MAX_THREADS(4)
 *                                      BINDER_SET_CONTEXT_MGR (handle 0)
 *   BC_ENTER_LOOPER                    BC_ENTER_LOOPER
 *   BC_TRANSACTION(handle 0)  ------>  BR_TRANSACTION (payload via mmap)
 *                                      verify + BC_REPLY + BC_FREE_BUFFER
 *   BR_TRANSACTION_COMPLETE            BR_TRANSACTION_COMPLETE
 *   BR_REPLY (verify payload)  <----
 *   BC_FREE_BUFFER
 *
 * One transaction (seq==1) carries a flat BINDER_TYPE_BINDER object: the
 * server must receive it as BINDER_TYPE_HANDLE, echo it back, and the
 * client must see its original binder pointer restored (cross-process
 * object translation). The client also answers the BR_INCREFS/BR_ACQUIRE
 * node handshake with BC_INCREFS_DONE/BC_ACQUIRE_DONE, like libbinder.
 *
 * Output contract:
 *   BINDER_PROBE RESULT: PASS n=<txns> version=8 obj=ok handshake=<k> ...
 *   BINDER_PROBE RESULT: FAIL <reason>          (exit 1)
 *   BINDER_PROBE RESULT: TIMEOUT                (exit 3)
 * Exit codes: 0 PASS, 1 FAIL, 2 setup error, 3 timeout.
 *
 * Compile: riscv64-linux-gnu-gcc -static -O2 -Wall -o binder_probe binder_probe.c
 * (fully static; runs under the minimal musl rootfs and as PID 1 via init=)
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

/* ---- minimal binder UAPI (numbers computed by the real _IOC macros) ---- */

typedef uint64_t binder_size_t;
typedef uint64_t binder_uintptr_t;

struct binder_write_read {
    binder_size_t write_size;
    binder_size_t write_consumed;
    binder_uintptr_t write_buffer;
    binder_size_t read_size;
    binder_size_t read_consumed;
    binder_uintptr_t read_buffer;
};

struct binder_version {
    int32_t protocol_version;
};

struct binder_transaction_data {
    union {
        uint32_t handle;
        binder_uintptr_t ptr;
    } target;
    binder_uintptr_t cookie;
    uint32_t code;
    uint32_t flags;
    int32_t sender_pid;
    uint32_t sender_euid;
    binder_size_t data_size;
    binder_size_t offsets_size;
    union {
        struct {
            binder_uintptr_t buffer;
            binder_uintptr_t offsets;
        } ptr;
        uint8_t buf[8];
    } data;
};

struct binder_ptr_cookie {
    binder_uintptr_t ptr;
    binder_uintptr_t cookie;
};

struct flat_binder_object {
    uint32_t type;
    uint32_t flags;
    union {
        binder_uintptr_t binder;
        uint32_t handle;
    };
    binder_uintptr_t cookie;
};

#define B_PACK_CHARS(c1, c2, c3, c4) \
    ((((c1) << 24)) | (((c2) << 16)) | (((c3) << 8)) | (c4))
#define B_TYPE_LARGE 0x85

enum {
    BINDER_TYPE_BINDER = B_PACK_CHARS('s', 'b', '*', B_TYPE_LARGE),
    BINDER_TYPE_WEAK_BINDER = B_PACK_CHARS('w', 'b', '*', B_TYPE_LARGE),
    BINDER_TYPE_HANDLE = B_PACK_CHARS('s', 'h', '*', B_TYPE_LARGE),
    BINDER_TYPE_WEAK_HANDLE = B_PACK_CHARS('w', 'h', '*', B_TYPE_LARGE),
};

#define BINDER_WRITE_READ _IOWR('b', 1, struct binder_write_read)
#define BINDER_SET_MAX_THREADS _IOW('b', 5, uint32_t)
#define BINDER_SET_CONTEXT_MGR _IOW('b', 7, int32_t)
#define BINDER_VERSION _IOWR('b', 9, struct binder_version)

enum {
    BC_TRANSACTION = _IOW('c', 0, struct binder_transaction_data),
    BC_REPLY = _IOW('c', 1, struct binder_transaction_data),
    BC_FREE_BUFFER = _IOW('c', 3, binder_uintptr_t),
    BC_INCREFS = _IOW('c', 4, uint32_t),
    BC_ACQUIRE = _IOW('c', 5, uint32_t),
    BC_RELEASE = _IOW('c', 6, uint32_t),
    BC_DECREFS = _IOW('c', 7, uint32_t),
    BC_INCREFS_DONE = _IOW('c', 8, struct binder_ptr_cookie),
    BC_ACQUIRE_DONE = _IOW('c', 9, struct binder_ptr_cookie),
    BC_REGISTER_LOOPER = _IO('c', 11),
    BC_ENTER_LOOPER = _IO('c', 12),
    BC_EXIT_LOOPER = _IO('c', 13),
    BC_DEAD_BINDER_DONE = _IOW('c', 16, binder_uintptr_t),
};

enum {
    BR_ERROR = _IOR('r', 0, int32_t),
    BR_OK = _IO('r', 1),
    BR_TRANSACTION = _IOR('r', 2, struct binder_transaction_data),
    BR_REPLY = _IOR('r', 3, struct binder_transaction_data),
    BR_DEAD_REPLY = _IO('r', 5),
    BR_TRANSACTION_COMPLETE = _IO('r', 6),
    BR_INCREFS = _IOR('r', 7, struct binder_ptr_cookie),
    BR_ACQUIRE = _IOR('r', 8, struct binder_ptr_cookie),
    BR_RELEASE = _IOR('r', 9, struct binder_ptr_cookie),
    BR_DECREFS = _IOR('r', 10, struct binder_ptr_cookie),
    BR_NOOP = _IO('r', 12),
    BR_SPAWN_LOOPER = _IO('r', 13),
    BR_FAILED_REPLY = _IO('r', 17),
};

/* ---- probe configuration ---- */

#define MAP_SIZE (1u << 20)   /* 1 MiB binder mmap (kernel caps at 4 MiB) */
#define SCRATCH_OFF 0x80000u /* parcel scratch inside the mapping */
#define READ_BUF_SZ 4096
#define WRITE_BUF_SZ 512
#define DATA_LEN 512
#define N_TXNS 64
#define CODE_ECHO 42
#define CODE_TERM 99
#define TXN_WITH_OBJECT 1 /* seq index that carries the binder object */

#define REQ_MAGIC 0xB1DEF00Du
#define REP_MAGIC 0xB2DEF00Du
#define OBJ_PTR_BASE 0x1234567800000000ull
#define OBJ_COOKIE 0xC00C1E00C0FFEE00ull

struct req_parcel {
    uint32_t magic;
    uint32_t seq;
    uint32_t len;
    uint32_t sum; /* FNV-1a over data[] */
    uint8_t data[DATA_LEN];
};

struct rep_parcel {
    uint32_t magic;
    uint32_t seq;
    uint32_t rsum; /* FNV-1a of the request data, transformed */
    uint32_t pad;
    uint8_t data[DATA_LEN];
};

static int g_fd = -1;
static uint8_t *g_map = NULL;
static const char *g_role = "?";

/* pending node-handshake answers, in BR arrival order:
 * [0] = BR_INCREFS (weak), [1] = BR_ACQUIRE (strong) */
static struct binder_ptr_cookie g_pending_done[2];
static int g_pending_done_n = 0;

static void on_alarm(int sig)
{
    (void)sig;
    printf("BINDER_PROBE RESULT: TIMEOUT (%s)\n", g_role);
    fflush(stdout);
    _exit(3);
}

static void die(const char *what)
{
    printf("BINDER_PROBE RESULT: FAIL %s: %s (%s, errno=%d)\n",
           what, strerror(errno), g_role, errno);
    fflush(stdout);
    _exit(1);
}

/* MemFree from /proc/meminfo, in kB (0 on parse failure) — used to watch
 * for binder-region/page leaks across repeated rounds. */
static long read_memfree(void)
{
    FILE *f = fopen("/proc/meminfo", "r");
    if (!f)
        return -1;
    char line[256];
    long kb = -1;
    while (fgets(line, sizeof(line), f)) {
        if (strncmp(line, "MemFree:", 8) == 0) {
            kb = strtol(line + 8, NULL, 10);
            break;
        }
    }
    fclose(f);
    return kb;
}

static uint32_t fnv1a(const uint8_t *p, size_t n)
{
    uint32_t h = 0x811c9dc5u;
    for (size_t i = 0; i < n; i++) {
        h ^= p[i];
        h *= 0x01000193u;
    }
    return h;
}

/* ---- BC command buffer ---- */

struct bcbuf {
    uint8_t b[WRITE_BUF_SZ];
    size_t len;
};

static void bc_reset(struct bcbuf *c) { c->len = 0; }

static void bc_put(struct bcbuf *c, const void *p, size_t n)
{
    if (c->len + n > sizeof(c->b))
        die("bc overflow");
    memcpy(c->b + c->len, p, n);
    c->len += n;
}

static void bc_u32(struct bcbuf *c, uint32_t v) { bc_put(c, &v, 4); }
static void bc_u64(struct bcbuf *c, uint64_t v) { bc_put(c, &v, 8); }

static void queue_node_dones(struct bcbuf *c)
{
    if (g_pending_done_n >= 1) {
        bc_u32(c, BC_INCREFS_DONE);
        bc_u64(c, g_pending_done[0].ptr);
        bc_u64(c, g_pending_done[0].cookie);
    }
    if (g_pending_done_n >= 2) {
        bc_u32(c, BC_ACQUIRE_DONE);
        bc_u64(c, g_pending_done[1].ptr);
        bc_u64(c, g_pending_done[1].cookie);
    }
    g_pending_done_n = 0;
}

/* ---- one BINDER_WRITE_READ round trip ---- */

struct br_seen {
    int complete;
    int spawns;
    int node_refs; /* BR_INCREFS/BR_ACQUIRE count */
    struct binder_transaction_data txn; /* valid if have_txn */
    struct binder_transaction_data reply; /* valid if have_reply */
    int have_txn, have_reply;
    int fatal;
};

static void br_dispatch(uint32_t cmd, const uint8_t *payload, struct br_seen *s)
{
    switch (cmd) {
    case BR_NOOP:
    case BR_OK:
        return;
    case BR_SPAWN_LOOPER:
        s->spawns++;
        return;
    case BR_TRANSACTION_COMPLETE:
        s->complete++;
        return;
    case BR_INCREFS:
    case BR_ACQUIRE: {
        struct binder_ptr_cookie pc;
        memcpy(&pc, payload, sizeof(pc));
        s->node_refs++;
        if (g_pending_done_n < 2)
            g_pending_done[g_pending_done_n++] = pc;
        return;
    }
    case BR_TRANSACTION:
        memcpy(&s->txn, payload, sizeof(s->txn));
        s->have_txn = 1;
        return;
    case BR_REPLY:
        memcpy(&s->reply, payload, sizeof(s->reply));
        s->have_reply = 1;
        return;
    default:
        printf("BINDER_PROBE RESULT: FAIL %s unexpected BR %08x\n", g_role, cmd);
        fflush(stdout);
        s->fatal = 1;
        return;
    }
}

/* Returns 0 on success (all write commands consumed). */
static int bwr_round(struct bcbuf *write, struct br_seen *seen)
{
    uint8_t read_buf[READ_BUF_SZ];
    struct binder_write_read bwr;
    memset(&bwr, 0, sizeof(bwr));
    bwr.write_size = write->len;
    bwr.write_buffer = (uintptr_t)write->b;
    bwr.read_size = sizeof(read_buf);
    bwr.read_consumed = 0;
    bwr.read_buffer = (uintptr_t)read_buf;

    if (ioctl(g_fd, BINDER_WRITE_READ, &bwr) < 0)
        return -1;
    if (bwr.write_consumed != bwr.write_size) {
        printf("BINDER_PROBE: write consumed %llu/%llu (%s)\n",
               (unsigned long long)bwr.write_consumed,
               (unsigned long long)bwr.write_size, g_role);
        return -1;
    }

    size_t off = 0;
    while (off + 4 <= bwr.read_consumed && !seen->fatal) {
        uint32_t cmd;
        memcpy(&cmd, read_buf + off, 4);
        off += 4;
        br_dispatch(cmd, read_buf + off, seen);
        if (cmd == BR_NOOP || cmd == BR_OK || cmd == BR_TRANSACTION_COMPLETE ||
            cmd == BR_DEAD_REPLY || cmd == BR_FAILED_REPLY ||
            cmd == BR_SPAWN_LOOPER)
            continue;
        if (cmd == BR_ERROR)
            off += 4;
        else if (cmd == BR_TRANSACTION || cmd == BR_REPLY)
            off += sizeof(struct binder_transaction_data);
        else if (cmd == BR_INCREFS || cmd == BR_ACQUIRE ||
                 cmd == BR_RELEASE || cmd == BR_DECREFS)
            off += sizeof(struct binder_ptr_cookie);
        else {
            printf("BINDER_PROBE: unknown BR %08x (%s)\n", cmd, g_role);
            return -1;
        }
    }
    return 0;
}

/* Write-only round (ENTER_LOOPER, FREE_BUFFER, handshake DONEs): these
 * never expect a response — a blocking read here would sleep forever,
 * like a libbinder write with bwr.read_size = 0. */
static int bwr_write_only(struct bcbuf *write)
{
    struct binder_write_read bwr;
    memset(&bwr, 0, sizeof(bwr));
    bwr.write_size = write->len;
    bwr.write_buffer = (uintptr_t)write->b;
    if (ioctl(g_fd, BINDER_WRITE_READ, &bwr) < 0)
        return -1;
    if (bwr.write_consumed != bwr.write_size)
        return -1;
    return 0;
}

/* ---- common setup ---- */

static void binder_setup(int become_mgr)
{
    g_fd = open("/dev/binder", O_RDWR | O_CLOEXEC);
    if (g_fd < 0)
        die("open /dev/binder");

    g_map = mmap(NULL, MAP_SIZE, PROT_READ | PROT_WRITE, MAP_SHARED, g_fd, 0);
    if (g_map == MAP_FAILED)
        die("mmap binder");

    struct binder_version v;
    v.protocol_version = -1;
    if (ioctl(g_fd, BINDER_VERSION, &v) < 0)
        die("BINDER_VERSION");
    if (v.protocol_version != 8)
        die("protocol version != 8");

    uint32_t max_threads = 4;
    if (ioctl(g_fd, BINDER_SET_MAX_THREADS, &max_threads) < 0)
        die("SET_MAX_THREADS");

    if (become_mgr) {
        int32_t zero = 0;
        if (ioctl(g_fd, BINDER_SET_CONTEXT_MGR, &zero) < 0)
            die("SET_CONTEXT_MGR");
    }

    struct bcbuf c;
    bc_reset(&c);
    bc_u32(&c, BC_ENTER_LOOPER);
    if (bwr_write_only(&c) < 0)
        die("ENTER_LOOPER");
}

/* ---- server (B): context manager loop ---- */

static int run_server(int ready_pipe)
{
    g_role = "server";
    signal(SIGALRM, on_alarm);
    alarm(120);

    binder_setup(1 /* become context manager */);
    if (write(ready_pipe, "R", 1) != 1)
        die("ready pipe");

    int served = 0, spawns = 0, completes = 0, replies_issued = 0;
    int obj_translated = 0, term_sent = 0;
    struct bcbuf reply_cmd; /* pending BC_REPLY + BC_FREE_BUFFER */
    int have_pending_reply = 0;

    while (!term_sent || completes < replies_issued) {
        struct bcbuf c;
        bc_reset(&c);
        queue_node_dones(&c);
        if (have_pending_reply) {
            bc_put(&c, reply_cmd.b, reply_cmd.len);
            have_pending_reply = 0;
        }

        struct br_seen s;
        memset(&s, 0, sizeof(s));
        if (bwr_round(&c, &s) < 0 || s.fatal)
            die("server bwr");
        completes += s.complete;
        spawns += s.spawns;

        if (s.have_txn) {
            served++;
            uint8_t *buf = (uint8_t *)(uintptr_t)s.txn.data.ptr.buffer;
            int with_obj = (s.txn.offsets_size == 8);

            /* handle-0 delivery: the context-manager node has ptr=cookie=0
             * and the sender must be our parent */
            if (s.txn.target.ptr != 0 || s.txn.cookie != 0)
                die("server: BR_TRANSACTION target not the context manager");
            if (s.txn.sender_pid != getppid())
                die("server: sender_pid mismatch");

            /* validate the request parcel (object sits at offset 0) */
            struct req_parcel *req =
                (struct req_parcel *)(buf + (with_obj ? sizeof(struct flat_binder_object) : 0));
            if (req->magic != REQ_MAGIC)
                die("server: request magic");
            if (req->len != DATA_LEN)
                die("server: request len");
            if (fnv1a(req->data, req->len) != req->sum)
                die("server: request checksum");

            if (with_obj) {
                uint64_t off0;
                memcpy(&off0, (void *)(uintptr_t)s.txn.data.ptr.offsets, 8);
                struct flat_binder_object *fo = (struct flat_binder_object *)(buf + off0);
                if (fo->type != BINDER_TYPE_HANDLE)
                    die("server: object not translated to HANDLE");
                obj_translated++;
            }

            /* build the reply in our mapping scratch */
            struct rep_parcel *rep = (struct rep_parcel *)(g_map + SCRATCH_OFF);
            rep->magic = REP_MAGIC;
            rep->seq = req->seq;
            rep->rsum = req->sum ^ 0xA5A5A5A5u;
            memcpy(rep->data, req->data, DATA_LEN);
            for (uint32_t i = 0; i < DATA_LEN; i++)
                rep->data[i] ^= 0x5A;

            size_t data_size = sizeof(*rep);
            uint64_t off0 = 0;
            size_t offsets_size = 0;
            if (with_obj) {
                /* echo the received object back (client gets its
                 * BINDER_TYPE_BINDER restored) */
                struct flat_binder_object echo;
                uint64_t roff;
                memcpy(&roff, (void *)(uintptr_t)s.txn.data.ptr.offsets, 8);
                memcpy(&echo, buf + roff, sizeof(echo));
                memmove(g_map + SCRATCH_OFF + sizeof(echo), rep, sizeof(*rep));
                memcpy(g_map + SCRATCH_OFF, &echo, sizeof(echo));
                data_size = sizeof(echo) + sizeof(*rep);
                offsets_size = 8;
            }

            struct binder_transaction_data rt;
            memset(&rt, 0, sizeof(rt));
            rt.code = s.txn.code;
            rt.flags = 0; /* synchronous reply */
            rt.data_size = data_size;
            rt.offsets_size = offsets_size;
            rt.data.ptr.buffer = (uintptr_t)(g_map + SCRATCH_OFF);
            rt.data.ptr.offsets = (uintptr_t)&off0;

            bc_reset(&reply_cmd);
            bc_u32(&reply_cmd, BC_REPLY);
            bc_put(&reply_cmd, &rt, sizeof(rt));
            bc_u32(&reply_cmd, BC_FREE_BUFFER);
            bc_u64(&reply_cmd, (uint64_t)(uintptr_t)buf);
            have_pending_reply = 1;
            replies_issued++;

            if (s.txn.code == CODE_TERM)
                term_sent = 1;
        }
    }

    printf("BINDER_PROBE SERVER DONE: served=%d spawns=%d obj=%d completes=%d\n",
           served, spawns, obj_translated, completes);
    fflush(stdout);
    munmap(g_map, MAP_SIZE);
    close(g_fd);
    return 0;
}

/* ---- client (A) ---- */

static int run_client(int ready_pipe)
{
    g_role = "client";
    signal(SIGALRM, on_alarm);
    alarm(120);

    binder_setup(0);
    char r = 0;
    if (read(ready_pipe, &r, 1) != 1 || r != 'R')
        die("ready pipe read");

    int ok = 0, completes = 0, frees = 0, obj_ok = 0, handshakes = 0;
    uint64_t expected_obj_ptr = OBJ_PTR_BASE | TXN_WITH_OBJECT;

    for (uint32_t seq = 0; seq <= N_TXNS; seq++) {
        uint32_t code = (seq == N_TXNS) ? CODE_TERM : CODE_ECHO;
        int with_obj = (seq == TXN_WITH_OBJECT);

        /* build the request parcel at the mapping scratch */
        uint8_t *base = g_map + SCRATCH_OFF;
        struct req_parcel *req;
        if (with_obj) {
            struct flat_binder_object fo;
            memset(&fo, 0, sizeof(fo));
            fo.type = BINDER_TYPE_BINDER;
            fo.binder = expected_obj_ptr;
            fo.cookie = OBJ_COOKIE;
            memcpy(base, &fo, sizeof(fo));
            req = (struct req_parcel *)(base + sizeof(fo));
        } else {
            req = (struct req_parcel *)base;
        }
        req->magic = REQ_MAGIC;
        req->seq = seq;
        req->len = DATA_LEN;
        for (uint32_t i = 0; i < DATA_LEN; i++)
            req->data[i] = (uint8_t)((seq * 131 + i * 7) & 0xff);
        req->sum = fnv1a(req->data, DATA_LEN);

        uint64_t off0 = 0;
        struct binder_transaction_data tr;
        memset(&tr, 0, sizeof(tr));
        tr.target.handle = 0; /* the context manager */
        tr.code = code;
        tr.flags = 0; /* synchronous */
        tr.data_size = (with_obj ? sizeof(struct flat_binder_object) : 0) + sizeof(*req);
        tr.offsets_size = with_obj ? 8 : 0;
        tr.data.ptr.buffer = (uintptr_t)base;
        tr.data.ptr.offsets = (uintptr_t)&off0;

        struct bcbuf c;
        bc_reset(&c);
        queue_node_dones(&c);
        bc_u32(&c, BC_TRANSACTION);
        bc_put(&c, &tr, sizeof(tr));

        /* send; keep reading (blocking) until BR_REPLY arrives */
        struct br_seen s;
        memset(&s, 0, sizeof(s));
        while (!s.have_reply) {
            struct br_seen r1;
            memset(&r1, 0, sizeof(r1));
            if (bwr_round(&c, &r1) < 0 || r1.fatal)
                die("client bwr");
            bc_reset(&c); /* write commands consumed on first round */
            s.complete += r1.complete;
            s.spawns += r1.spawns;
            if (r1.node_refs)
                s.node_refs += r1.node_refs;
            if (r1.have_reply) {
                s.reply = r1.reply;
                s.have_reply = 1;
                break;
            }
            if (r1.have_txn)
                die("client got BR_TRANSACTION");
        }
        completes += s.complete;

        /* verify the reply payload */
        uint8_t *rbuf = (uint8_t *)(uintptr_t)s.reply.data.ptr.buffer;
        struct rep_parcel *rep;
        struct flat_binder_object *rfo = NULL;
        if (s.reply.offsets_size == 8) {
            rfo = (struct flat_binder_object *)rbuf;
            rep = (struct rep_parcel *)(rbuf + sizeof(*rfo));
        } else {
            rep = (struct rep_parcel *)rbuf;
        }
        if (rep->magic != REP_MAGIC || rep->seq != seq)
            die("reply magic/seq");
        if (rep->rsum != (req->sum ^ 0xA5A5A5A5u))
            die("reply checksum");
        for (uint32_t i = 0; i < DATA_LEN; i++)
            if (rep->data[i] != (uint8_t)(((seq * 131 + i * 7) & 0xff) ^ 0x5A))
                die("reply payload bytes");

        if (with_obj) {
            if (!rfo || rfo->type != BINDER_TYPE_BINDER)
                die("reply object not restored to BINDER_TYPE_BINDER");
            if (rfo->binder != expected_obj_ptr || rfo->cookie != OBJ_COOKIE)
                die("reply object ptr/cookie mismatch");
            obj_ok = 1;
        }
        if (s.node_refs)
            handshakes += s.node_refs;

        /* free the reply buffer */
        bc_reset(&c);
        bc_u32(&c, BC_FREE_BUFFER);
        bc_u64(&c, (uint64_t)(uintptr_t)rbuf);
        if (bwr_write_only(&c) < 0)
            die("client free bwr");
        frees++;
        ok++;
    }

    /* flush any pending node dones so the driver state settles */
    struct bcbuf c;
    bc_reset(&c);
    queue_node_dones(&c);
    if (c.len) {
        if (bwr_write_only(&c) < 0)
            die("client final dones");
    }

    int st = -1;
    if (waitpid(-1, &st, 0) < 0)
        die("waitpid");
    if (!WIFEXITED(st) || WEXITSTATUS(st) != 0) {
        printf("BINDER_PROBE RESULT: FAIL server exit status %d\n", st);
        fflush(stdout);
        return 1;
    }
    if (!obj_ok)
        die("object round-trip missing");

    printf("BINDER_PROBE ROUND-PASS n=%d version=8 obj=%s handshakes=%d "
           "completes=%d frees=%d\n",
           ok, obj_ok ? "ok" : "none", handshakes, completes, frees);
    fflush(stdout);
    /* Release the mapping BEFORE the fd: munmap drops the per-page
     * mapping references, close drops the owner reference — the region
     * pages return to the buddy allocator only when both are gone. */
    munmap(g_map, MAP_SIZE);
    g_map = NULL;
    close(g_fd);
    g_fd = -1;
    return 0;
}

#define ROUNDS 5

int main(void)
{
    setvbuf(stdout, NULL, _IONBF, 0);

    long mem0 = read_memfree();
    for (int round = 0; round < ROUNDS; round++) {
        int p2a[2];
        if (pipe(p2a) < 0) {
            printf("BINDER_PROBE RESULT: FAIL pipe errno=%d\n", errno);
            return 2;
        }
        pid_t pid = fork();
        if (pid < 0) {
            printf("BINDER_PROBE RESULT: FAIL fork errno=%d\n", errno);
            return 2;
        }
        if (pid == 0) {
            close(p2a[0]);
            _exit(run_server(p2a[1]));
        }
        close(p2a[1]);
        int rc = run_client(p2a[0]);
        if (rc != 0)
            return rc;
        long mf = read_memfree();
        printf("BINDER_PROBE ROUND %d/%d memfree=%ld kB\n", round + 1, ROUNDS, mf);
        if (mem0 >= 0 && mf >= 0 && round >= 1) {
            /* two binder mmap regions (2 MiB) may legitimately still be
             * settling in round 0; later rounds must be stable */
            static long prev = -1;
            static long first = -1;
            if (prev >= 0 && prev - mf > 2048) {
                printf("BINDER_PROBE RESULT: FAIL memory drift %ld -> %ld kB\n", prev, mf);
                return 1;
            }
            if (first < 0)
                first = mf;
            prev = mf;
            if (round == ROUNDS - 1)
                printf("BINDER_PROBE RESULT: PASS rounds=%d memfree_drift=%ld kB\n",
                       ROUNDS, first - mf);
        }
    }
    return 0;
}
