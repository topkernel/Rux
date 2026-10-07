/* binder2_probe.c — binder S2: samgr-style service lifecycle probe.
 *
 * Three processes over /dev/binder, mimicking OpenHarmony samgr usage:
 *
 *   samgr (context manager)      service (SystemAbility stub)
 *   ------------------------     -------------------------------
 *   SET_CONTEXT_MGR             creates local binder object (ptr/cookie)
 *   loop: ADD/GET/TERM          ADD_SERVICE: txn(handle 0) w/ object
 *   death-watch every handle    loop: ECHO pings, reply, exit on TERM
 *
 *   client (proxy user)
 *   ----------------------------
 *   GET_SERVICE -> reply carries BINDER_TYPE_HANDLE -> desc
 *   node handshake (BC_INCREFS_DONE / BC_ACQUIRE_DONE)
 *   ECHO ping via desc (twoway, validates the handle)
 *   BC_REQUEST_DEATH_NOTIFICATION(desc, B) + CLEAR -> BR_CLEAR_DEATH_NOTIFICATION_DONE
 *   BC_REQUEST_DEATH_NOTIFICATION(desc, A)
 *   TERM the service; it exits ->
 *        BR_DEAD_BINDER(A) -> BC_DEAD_BINDER_DONE
 *        (B must NOT arrive — it was cleared)
 *   txn to the dead handle -> BR_DEAD_REPLY
 *   CLEAR(A) -> CLEAR_DONE(A); REQUEST on the dead handle (C) ->
 *        immediate BR_DEAD_BINDER(C) -> DONE(C)
 *
 * Death notifications can arrive bundled with any read round (e.g. the
 * reply of the very transaction that killed the service); like libbinder's
 * processPendingDerefs, arrivals are recorded globally and matched later.
 *
 * samgr additionally death-watches the registered handle and reports its
 * own deliveries; the client is the PASS/FAIL oracle.
 *
 * Output contract:
 *   B2 PROBE RESULT: PASS rounds=<n> ...   (exit 0)
 *   B2 PROBE RESULT: FAIL <reason>         (exit 1)
 *   B2 PROBE RESULT: TIMEOUT               (exit 3)
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
    BINDER_TYPE_FD = B_PACK_CHARS('f', 'd', '*', B_TYPE_LARGE),
    BINDER_TYPE_FDA = B_PACK_CHARS('f', 'd', 'a', B_TYPE_LARGE),
    BINDER_TYPE_PTR = B_PACK_CHARS('p', 't', '*', B_TYPE_LARGE),
};

struct binder_buffer_object {
    uint32_t hdr_type;
    uint32_t flags;
    uint64_t buffer;
    uint64_t length;
    uint64_t parent;
    uint64_t parent_offset;
};

struct binder_fd_array_object {
    uint32_t hdr_type;
    uint32_t flags;
    uint64_t num_fds;
    uint64_t parent;
    uint64_t parent_offset;
};

#define FLAT_BINDER_FLAG_ACCEPTS_FDS 0x100
#define BINDER_BUFFER_FLAG_HAS_PARENT 0x01

#define BINDER_WRITE_READ _IOWR('b', 1, struct binder_write_read)
#define BINDER_SET_MAX_THREADS _IOW('b', 5, uint32_t)
#define BINDER_SET_CONTEXT_MGR _IOW('b', 7, int32_t)
#define BINDER_VERSION _IOWR('b', 9, struct binder_version)

struct binder_version {
    int32_t protocol_version;
};

struct binder_transaction_data_sg {
    struct binder_transaction_data data;
    binder_size_t buffers_size;
};

enum {
    BC_TRANSACTION = _IOW('c', 0, struct binder_transaction_data),
    BC_TRANSACTION_SG = _IOW('c', 17, struct binder_transaction_data_sg),
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
    BC_REQUEST_DEATH_NOTIFICATION = _IOW('c', 14, struct binder_ptr_cookie),
    BC_CLEAR_DEATH_NOTIFICATION = _IOW('c', 15, struct binder_ptr_cookie),
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
    BR_DEAD_BINDER = _IOR('r', 11, binder_uintptr_t),
    BR_CLEAR_DEATH_NOTIFICATION_DONE = _IOR('r', 14, binder_uintptr_t),
    BR_NOOP = _IO('r', 12),
    BR_SPAWN_LOOPER = _IO('r', 13),
    BR_FAILED_REPLY = _IO('r', 17),
};

/* ---- configuration ---- */

#define MAP_SIZE (1u << 20)
#define SCRATCH_OFF 0x80000u
#define SCRATCH_OFF2 0x81000u /* disjoint staging area (overlap-safe) */
#define READ_BUF_SZ 4096
#define WRITE_BUF_SZ 512
#define DATA_LEN 256

#define CODE_ADD 100
#define CODE_GET 101
#define CODE_ECHO 102
#define CODE_TERM 103
#define CODE_SENDFD 104 /* parcel carries a BINDER_TYPE_FD */
#define CODE_SENDRAW 105 /* BC_TRANSACTION_SG with PTR + FDA */
#define TF_ONE_WAY 0x01

#define REQ_MAGIC 0xB1D2F00Du
#define REP_MAGIC 0xB2D2F00Du

#define SVC_PTR 0x5678000000000001ull
#define SVC_COOKIE 0x0BADF00D0BADF00Dull

#define COOKIE_A 0xD1E0000000000001ull /* live death watch */
#define COOKIE_B 0xD1E0000000000002ull /* cleared before death */
#define COOKIE_C 0xD1E0000000000003ull /* requested after death */
#define SAMGR_COOKIE_BASE 0x5A67000000000000ull /* samgr watches */

struct req_parcel {
    uint32_t magic;
    uint32_t len;
    uint32_t sum;
    uint32_t pad;
    uint8_t data[DATA_LEN];
};

struct rep_parcel {
    uint32_t magic;
    uint32_t val;
    uint32_t sum2;
    uint32_t pad;
    uint8_t data[DATA_LEN];
};

static int g_fd = -1;
static uint8_t *g_map = NULL;
static const char *g_role = "?";
static const char *g_dev = "/dev/binder";

/* pending node-handshake answers in BR arrival order */
static struct binder_ptr_cookie g_pending_done[2];
static int g_pending_done_n = 0;

/* Death notifications and clear-dones can arrive bundled with ANY read
 * round (e.g. the reply of the very transaction that killed the service)
 * — record them globally like libbinder's pending derefs, then match. */
/* Delivered transactions queue: one read round can stage several
 * BR_TRANSACTIONs; like libbinder's executeCommand loop they must all be
 * handled, not just the last one. */
static struct binder_transaction_data g_pending_txn[8];
static int g_pending_txn_n = 0;

static void note_txn(struct binder_transaction_data *td)
{
    if (g_pending_txn_n < 8)
        g_pending_txn[g_pending_txn_n++] = *td;
}

static int take_txn(struct binder_transaction_data *td)
{
    if (g_pending_txn_n == 0)
        return 0;
    *td = g_pending_txn[0];
    memmove(g_pending_txn, g_pending_txn + 1, sizeof(g_pending_txn[0]) * (size_t)(g_pending_txn_n - 1));
    g_pending_txn_n--;
    return 1;
}

static uint64_t g_dead_seen[8];
static int g_dead_seen_n = 0;
static uint64_t g_clear_seen[8];
static int g_clear_seen_n = 0;

static void note_death(uint64_t cookie)
{
    if (g_dead_seen_n < 8)
        g_dead_seen[g_dead_seen_n++] = cookie;
}

static void note_clear_done(uint64_t cookie)
{
    if (g_clear_seen_n < 8)
        g_clear_seen[g_clear_seen_n++] = cookie;
}

static int take_death(uint64_t cookie)
{
    for (int i = 0; i < g_dead_seen_n; i++) {
        if (g_dead_seen[i] == cookie) {
            g_dead_seen[i] = g_dead_seen[--g_dead_seen_n];
            return 1;
        }
    }
    return 0;
}

static int take_clear_done(uint64_t cookie)
{
    for (int i = 0; i < g_clear_seen_n; i++) {
        if (g_clear_seen[i] == cookie) {
            g_clear_seen[i] = g_clear_seen[--g_clear_seen_n];
            return 1;
        }
    }
    return 0;
}

static void on_alarm(int sig)
{
    (void)sig;
    printf("B2 PROBE RESULT: TIMEOUT (%s)\n", g_role);
    fflush(stdout);
    _exit(3);
}

static void die(const char *what)
{
    printf("B2 PROBE RESULT: FAIL %s: %s (%s, errno=%d)\n",
           what, strerror(errno), g_role, errno);
    fflush(stdout);
    _exit(1);
}

static void die2(const char *what)
{
    printf("B2 PROBE RESULT: FAIL %s (%s)\n", what, g_role);
    fflush(stdout);
    _exit(1);
}

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

/* ---- BR dispatch ---- */

struct br_seen {
    int complete;
    int dead_reply;
    int failed_reply;
    int node_refs;
    uint64_t dead_cookie;
    int have_dead;
    uint64_t clear_done_cookie;
    int have_clear_done;
    struct binder_transaction_data txn;
    struct binder_transaction_data reply;
    int have_txn, have_reply;
    int fatal;
};

static size_t br_payload(uint32_t cmd)
{
    switch (cmd) {
    case BR_TRANSACTION:
    case BR_REPLY:
        return sizeof(struct binder_transaction_data);
    case BR_INCREFS:
    case BR_ACQUIRE:
    case BR_RELEASE:
    case BR_DECREFS:
    case BR_DEAD_BINDER:
    case BR_CLEAR_DEATH_NOTIFICATION_DONE:
        return sizeof(struct binder_ptr_cookie);
    case BR_ERROR:
        return 4;
    default:
        return 0;
    }
}

static void br_dispatch(uint32_t cmd, const uint8_t *payload, struct br_seen *s)
{
    struct binder_ptr_cookie pc;
    uint64_t ck;
    switch (cmd) {
    case BR_NOOP:
    case BR_OK:
    case BR_SPAWN_LOOPER:
        return;
    case BR_TRANSACTION_COMPLETE:
        s->complete++;
        return;
    case BR_DEAD_REPLY:
        s->dead_reply++;
        return;
    case BR_FAILED_REPLY:
        s->failed_reply++;
        return;
    case BR_INCREFS:
    case BR_ACQUIRE:
        memcpy(&pc, payload, sizeof(pc));
        s->node_refs++;
        if (g_pending_done_n < 2)
            g_pending_done[g_pending_done_n++] = pc;
        return;
    case BR_RELEASE:
    case BR_DECREFS:
        return;
    case BR_DEAD_BINDER:
        memcpy(&ck, payload, 8);
        s->dead_cookie = ck;
        s->have_dead++;
        note_death(ck);
        return;
    case BR_CLEAR_DEATH_NOTIFICATION_DONE:
        memcpy(&ck, payload, 8);
        s->clear_done_cookie = ck;
        s->have_clear_done++;
        note_clear_done(ck);
        return;
    case BR_TRANSACTION:
        memcpy(&s->txn, payload, sizeof(s->txn));
        s->have_txn = 1;
        note_txn(&s->txn);
        return;
    case BR_REPLY:
        memcpy(&s->reply, payload, sizeof(s->reply));
        s->have_reply = 1;
        return;
    default:
        printf("B2 PROBE RESULT: FAIL %s unexpected BR %08x\n", g_role, cmd);
        fflush(stdout);
        s->fatal = 1;
        return;
    }
}

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
        printf("B2 PROBE: write consumed %llu/%llu (%s)\n",
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
        off += br_payload(cmd);
    }
    return 0;
}

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

/* blocking read-only round */
static int bwr_read_only(struct br_seen *seen)
{
    struct bcbuf c;
    bc_reset(&c);
    queue_node_dones(&c);
    return bwr_round(&c, seen);
}

/* Blocking wait until the given death cookie has been seen, then consume
 * it and acknowledge with BC_DEAD_BINDER_DONE. */
static int wait_death(uint64_t cookie)
{
    while (!take_death(cookie)) {
        struct br_seen s;
        memset(&s, 0, sizeof(s));
        if (bwr_read_only(&s) < 0 || s.fatal)
            return -1;
    }
    struct bcbuf c;
    bc_reset(&c);
    bc_u32(&c, BC_DEAD_BINDER_DONE);
    bc_u64(&c, cookie);
    return bwr_write_only(&c);
}

static int wait_clear_done(uint64_t cookie)
{
    while (!take_clear_done(cookie)) {
        struct br_seen s;
        memset(&s, 0, sizeof(s));
        if (bwr_read_only(&s) < 0 || s.fatal)
            return -1;
    }
    return 0;
}

static void binder_setup(int become_mgr)
{
    g_fd = open(g_dev, O_RDWR | O_CLOEXEC);
    if (g_fd < 0)
        die("open binder device");

    g_map = mmap(NULL, MAP_SIZE, PROT_READ | PROT_WRITE, MAP_SHARED, g_fd, 0);
    if (g_map == MAP_FAILED)
        die("mmap binder");

    struct binder_version v;
    v.protocol_version = -1;
    if (ioctl(g_fd, BINDER_VERSION, &v) < 0)
        die("BINDER_VERSION");
    if (v.protocol_version != 8)
        die("protocol version != 8");

    uint32_t maxThreads = 4;
    if (ioctl(g_fd, BINDER_SET_MAX_THREADS, &maxThreads) < 0)
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

/* Build a data+offsets parcel carrying one object + req payload. */
static size_t build_obj_req(uint32_t type, uint64_t ptr_or_handle, uint64_t cookie,
                            uint32_t seq, uint64_t *off0_out)
{
    struct flat_binder_object fo;
    memset(&fo, 0, sizeof(fo));
    fo.type = type;
    if (type == BINDER_TYPE_BINDER)
        fo.binder = ptr_or_handle;
    else
        fo.handle = (uint32_t)ptr_or_handle;
    fo.cookie = cookie;
    memcpy(g_map + SCRATCH_OFF, &fo, sizeof(fo));

    struct req_parcel *req = (struct req_parcel *)(g_map + SCRATCH_OFF + sizeof(fo));
    req->magic = REQ_MAGIC;
    req->len = DATA_LEN;
    req->pad = seq;
    for (uint32_t i = 0; i < DATA_LEN; i++)
        req->data[i] = (uint8_t)((seq * 131 + i * 7) & 0xff);
    req->sum = fnv1a(req->data, DATA_LEN);

    *off0_out = 0;
    return sizeof(fo) + sizeof(*req);
}

static size_t build_plain_req(uint32_t seq)
{
    struct req_parcel *req = (struct req_parcel *)(g_map + SCRATCH_OFF);
    req->magic = REQ_MAGIC;
    req->len = DATA_LEN;
    req->pad = seq;
    for (uint32_t i = 0; i < DATA_LEN; i++)
        req->data[i] = (uint8_t)((seq * 131 + i * 7) & 0xff);
    req->sum = fnv1a(req->data, DATA_LEN);
    return sizeof(*req);
}

/* Send a sync transaction and block until BR_REPLY (or DEAD/FAILED). */
static int do_txn(uint32_t handle, uint32_t code, size_t data_size, size_t offsets_size,
                  struct br_seen *out)
{
    uint64_t off0 = 0;
    struct binder_transaction_data tr;
    memset(&tr, 0, sizeof(tr));
    tr.target.handle = handle;
    tr.code = code;
    tr.flags = 0x10; /* TF_ACCEPT_FDS */
    tr.data_size = data_size;
    tr.offsets_size = offsets_size;
    tr.data.ptr.buffer = (uintptr_t)(g_map + SCRATCH_OFF);
    tr.data.ptr.offsets = (uintptr_t)&off0;

    struct bcbuf c;
    bc_reset(&c);
    queue_node_dones(&c);
    bc_u32(&c, BC_TRANSACTION);
    bc_put(&c, &tr, sizeof(tr));

    memset(out, 0, sizeof(*out));
    while (!out->have_reply && !out->dead_reply && !out->failed_reply && !out->fatal) {
        struct br_seen r1;
        memset(&r1, 0, sizeof(r1));
        if (bwr_round(&c, &r1) < 0)
            return -1;
        bc_reset(&c);
        queue_node_dones(&c);
        out->complete += r1.complete;
        out->node_refs += r1.node_refs;
        if (r1.have_reply) {
            out->reply = r1.reply;
            out->have_reply = 1;
            break;
        }
        if (r1.dead_reply || r1.failed_reply) {
            out->dead_reply += r1.dead_reply;
            out->failed_reply += r1.failed_reply;
            break;
        }
    }
    return 0;
}

/* Send a sync BC_TRANSACTION_SG with extra sg payload. */
static int do_txn_sg(uint32_t handle, uint32_t code, size_t data_size, size_t offsets_size,
                     size_t buffers_size, struct br_seen *out)
{
    uint64_t offs[4] = {0, 40, 0, 0};
    struct binder_transaction_data_sg tsg;
    memset(&tsg, 0, sizeof(tsg));
    tsg.data.target.handle = handle;
    tsg.data.code = code;
    tsg.data.flags = 0x10; /* TF_ACCEPT_FDS */
    tsg.data.data_size = data_size;
    tsg.data.offsets_size = offsets_size;
    tsg.data.data.ptr.buffer = (uintptr_t)(g_map + SCRATCH_OFF);
    tsg.data.data.ptr.offsets = (uintptr_t)&offs;
    tsg.buffers_size = buffers_size;

    struct bcbuf c;
    bc_reset(&c);
    queue_node_dones(&c);
    bc_u32(&c, BC_TRANSACTION_SG);
    bc_put(&c, &tsg, sizeof(tsg));

    memset(out, 0, sizeof(*out));
    while (!out->have_reply && !out->dead_reply && !out->failed_reply && !out->fatal) {
        struct br_seen r1;
        memset(&r1, 0, sizeof(r1));
        if (bwr_round(&c, &r1) < 0)
            return -1;
        bc_reset(&c);
        queue_node_dones(&c);
        out->complete += r1.complete;
        out->node_refs += r1.node_refs;
        if (r1.have_reply) {
            out->reply = r1.reply;
            out->have_reply = 1;
            break;
        }
        if (r1.dead_reply || r1.failed_reply) {
            out->dead_reply += r1.dead_reply;
            out->failed_reply += r1.failed_reply;
            break;
        }
    }
    return 0;
}

static int free_buffer(uint64_t ptr)
{
    struct bcbuf c;
    bc_reset(&c);
    bc_u32(&c, BC_FREE_BUFFER);
    bc_u64(&c, ptr);
    return bwr_write_only(&c);
}

/* ---- samgr: the context manager ---- */

static int run_samgr(int ready_pipe)
{
    g_role = "samgr";
    signal(SIGALRM, on_alarm);
    alarm(180);

    binder_setup(1);
    if (write(ready_pipe, "R", 1) != 1)
        die("samgr ready pipe");

    uint32_t svc_handle = 0;
    int deaths_seen = 0;
    int served = 0;
    int term = 0;

    while (!term) {
        struct br_seen s;
        memset(&s, 0, sizeof(s));
        struct bcbuf c;
        bc_reset(&c);
        queue_node_dones(&c);
        if (bwr_round(&c, &s) < 0 || s.fatal)
            die("samgr bwr");

        if (s.have_dead) {
            if (s.dead_cookie != SAMGR_COOKIE_BASE + 1)
                die2("samgr: unexpected death cookie");
            deaths_seen++;
            struct bcbuf d;
            bc_reset(&d);
            bc_u32(&d, BC_DEAD_BINDER_DONE);
            bc_u64(&d, s.dead_cookie);
            if (bwr_write_only(&d) < 0)
                die("samgr DONE");
            /* drop the dead handle refs (strong-only: RELEASE retires it) */
            bc_reset(&d);
            bc_u32(&d, BC_RELEASE);
            bc_u32(&d, svc_handle);
            if (bwr_write_only(&d) < 0)
                die("samgr release");
            continue;
        }
        if (s.have_clear_done)
            die2("samgr: unexpected CLEAR_DONE");

        struct binder_transaction_data td;
        while (take_txn(&td)) {
            s.txn = td;
            served++;
            uint8_t *buf = (uint8_t *)(uintptr_t)s.txn.data.ptr.buffer;
            uint32_t code = s.txn.code;

            /* Build the reply body in the disjoint staging area; the
             * final parcel is assembled at SCRATCH_OFF afterwards (the
             * flat object must not be clobbered by rep field writes). */
            struct rep_parcel *rep = (struct rep_parcel *)(g_map + SCRATCH_OFF2);
            rep->magic = REP_MAGIC;
            memset(rep->data, 0xEE, DATA_LEN);

            size_t data_size = sizeof(*rep);
            size_t offsets_size = 0;
            uint64_t off0 = 0;
            int ok = 1;

            if (code == CODE_ADD) {
                if (s.txn.offsets_size != 8)
                    die2("samgr: ADD without object");
                uint64_t roff;
                memcpy(&roff, (void *)(uintptr_t)s.txn.data.ptr.offsets, 8);
                struct flat_binder_object *fo = (struct flat_binder_object *)(buf + roff);
                if (fo->type != BINDER_TYPE_HANDLE)
                    die2("samgr: ADD object not a HANDLE");
                svc_handle = fo->handle;
                /* samgr watches the service for death */
                struct bcbuf d;
                bc_reset(&d);
                bc_u32(&d, BC_REQUEST_DEATH_NOTIFICATION);
                bc_u32(&d, svc_handle);
                bc_u64(&d, SAMGR_COOKIE_BASE + 1);
                if (bwr_write_only(&d) < 0)
                    die("samgr request death");
                rep->val = 1;
            } else if (code == CODE_GET) {
                if (!svc_handle)
                    die2("samgr: GET before ADD");
                rep->val = 2;
            } else if (code == CODE_TERM) {
                rep->val = 3;
                term = 1;
            } else {
                ok = 0;
            }

            rep->sum2 = ok ? 0x600D600Du : 0xDEADDEADu;
            if (code == CODE_GET) {
                struct flat_binder_object fo;
                memset(&fo, 0, sizeof(fo));
                fo.type = BINDER_TYPE_HANDLE;
                fo.handle = svc_handle;
                memcpy(g_map + SCRATCH_OFF, &fo, sizeof(fo));
                memcpy(g_map + SCRATCH_OFF + sizeof(fo), rep, sizeof(*rep));
                data_size = sizeof(fo) + sizeof(*rep);
                offsets_size = 8;
            } else {
                memcpy(g_map + SCRATCH_OFF, rep, sizeof(*rep));
            }

            struct binder_transaction_data rt;
            memset(&rt, 0, sizeof(rt));
            rt.code = s.txn.code;
            rt.flags = 0;
            rt.data_size = data_size;
            rt.offsets_size = offsets_size;
            rt.data.ptr.buffer = (uintptr_t)(g_map + SCRATCH_OFF);
            rt.data.ptr.offsets = (uintptr_t)&off0;

            struct bcbuf r;
            bc_reset(&r);
            bc_u32(&r, BC_REPLY);
            bc_put(&r, &rt, sizeof(rt));
            bc_u32(&r, BC_FREE_BUFFER);
            bc_u64(&r, (uint64_t)(uintptr_t)buf);
            if (bwr_write_only(&r) < 0)
                die("samgr reply");
        }
    }

    printf("B2 SAMGR DONE: served=%d deaths=%d\n", served, deaths_seen);
    fflush(stdout);
    munmap(g_map, MAP_SIZE);
    close(g_fd);
    return deaths_seen == 1 ? 0 : 1;
}

/* ---- service: registers SVC_PTR, answers pings, exits on TERM ---- */

static int run_service(int ready_pipe, int ok_pipe)
{
    g_role = "service";
    signal(SIGALRM, on_alarm);
    alarm(180);

    binder_setup(0);
    char r = 0;
    if (read(ready_pipe, &r, 1) != 1 || r != 'R')
        die("service ready read");

    /* service-side pipe whose write end travels back to the client in
     * the CODE_SENDFD reply (cross-process fd install on the reply path) */
    int pipeY[2];
    if (pipe(pipeY) < 0)
        die("service pipeY");

    /* register: ADD via handle 0 carrying our binder object (registered
     * with FLAT_BINDER_FLAG_ACCEPTS_FDS so it may receive fds) */
    {
        uint64_t off0 = 0;
        size_t ds = build_obj_req(BINDER_TYPE_BINDER, SVC_PTR, SVC_COOKIE, 7, &off0);
        struct flat_binder_object *reg = (struct flat_binder_object *)(g_map + SCRATCH_OFF);
        reg->flags = FLAT_BINDER_FLAG_ACCEPTS_FDS;
        struct br_seen s;
        if (do_txn(0, CODE_ADD, ds, 8, &s) < 0)
            die("service add txn");
        if (!s.have_reply)
            die2("service: no reply to ADD");
        uint8_t *rbuf = (uint8_t *)(uintptr_t)s.reply.data.ptr.buffer;
        struct rep_parcel *rep = (struct rep_parcel *)rbuf;
        if (rep->magic != REP_MAGIC || rep->sum2 != 0x600D600Du)
            die2("service: ADD reply bad");
        if (free_buffer((uint64_t)(uintptr_t)rbuf) < 0)
            die("service free");
    }
    if (write(ok_pipe, "S", 1) != 1)
        die("service ok pipe");

    int pings = 0;
    int oneways = 0;
    while (1) {
        struct br_seen s;
        memset(&s, 0, sizeof(s));
        struct bcbuf c;
        bc_reset(&c);
        queue_node_dones(&c);
        if (bwr_round(&c, &s) < 0 || s.fatal)
            die("service bwr");

        if (s.have_dead)
            die2("service: unexpected BR_DEAD_BINDER");
        if (s.have_clear_done)
            die2("service: unexpected CLEAR_DONE");

        struct binder_transaction_data td;
        while (take_txn(&td)) {
            s.txn = td;
            if (s.txn.target.ptr != SVC_PTR || s.txn.cookie != SVC_COOKIE)
                die2("service: txn target not our object");
            if (s.txn.flags & TF_ONE_WAY) {
                /* oneway: no reply, no txn stack — just count and free */
                uint8_t *obuf = (uint8_t *)(uintptr_t)s.txn.data.ptr.buffer;
                struct req_parcel *oreq = (struct req_parcel *)obuf;
                if (oreq->magic != REQ_MAGIC || fnv1a(oreq->data, oreq->len) != oreq->sum)
                    die2("service: bad oneway parcel");
                oneways++;
                struct bcbuf f;
                bc_reset(&f);
                bc_u32(&f, BC_FREE_BUFFER);
                bc_u64(&f, (uint64_t)(uintptr_t)obuf);
                if (bwr_write_only(&f) < 0)
                    die("service oneway free");
                continue;
            }
            uint8_t *buf = (uint8_t *)(uintptr_t)s.txn.data.ptr.buffer;
            uint32_t code = s.txn.code;
            size_t hdr = 0; /* leading object bytes before the req parcel */
            int reply_with_fd = -1; /* fd for the reply object, if >= 0 */

            if (code == CODE_SENDFD) {
                if (s.txn.offsets_size != 8)
                    die2("service: SENDFD offsets");
                uint64_t roff;
                memcpy(&roff, (void *)(uintptr_t)s.txn.data.ptr.offsets, 8);
                struct flat_binder_object *fo = (struct flat_binder_object *)(buf + roff);
                if (fo->type != BINDER_TYPE_FD)
                    die2("service: SENDFD object not an FD");
                /* write through the installed fd: the client reads it on
                 * its own read end — proves it is the same open file */
                if (write((int)fo->handle, "S", 1) != 1)
                    die2("service: write via translated fd");
                reply_with_fd = pipeY[1];
                hdr = sizeof(*fo);
            } else if (code == CODE_SENDRAW) {
                if (s.txn.offsets_size != 16)
                    die2("service: SENDRAW offsets");
                uint64_t offs[2];
                memcpy(offs, (void *)(uintptr_t)s.txn.data.ptr.offsets, 16);
                struct binder_buffer_object *bp = (struct binder_buffer_object *)(buf + offs[0]);
                struct binder_fd_array_object *fda = (struct binder_fd_array_object *)(buf + offs[1]);
                if (bp->hdr_type != BINDER_TYPE_PTR || fda->hdr_type != BINDER_TYPE_FDA)
                    die2("service: SENDRAW object types");
                if (bp->length != 64)
                    die2("service: SENDRAW ptr length");
                /* the PTR's buffer must now point INTO our own mapping */
                uint8_t *raw = (uint8_t *)(uintptr_t)bp->buffer;
                if (raw < g_map || raw + 64 > g_map + MAP_SIZE)
                    die2("service: SENDRAW ptr not fixed up to our mapping");
                if (memcmp(raw, "RAWDATA", 7) != 0 || memcmp(raw + 24, "TAILDATA", 8) != 0)
                    die2("service: SENDRAW pattern mismatch");
                if (fda->num_fds != 2 || fda->parent != 0 || fda->parent_offset != 16)
                    die2("service: SENDRAW fda meta");
                uint32_t fd0, fd1;
                memcpy(&fd0, raw + 16, 4);
                memcpy(&fd1, raw + 20, 4);
                if (write((int)fd0, "1", 1) != 1 || write((int)fd1, "2", 1) != 1)
                    die2("service: write via fda fds");
                hdr = sizeof(*bp) + sizeof(*fda);
            } else if (s.txn.offsets_size == 8) {
                uint64_t roff;
                memcpy(&roff, (void *)(uintptr_t)s.txn.data.ptr.offsets, 8);
                struct flat_binder_object *fo = (struct flat_binder_object *)(buf + roff);
                if (fo->type != BINDER_TYPE_BINDER || fo->binder != SVC_PTR)
                    die2("service: echo object not restored");
                hdr = sizeof(*fo);
            }

            struct req_parcel *req = (struct req_parcel *)(buf + hdr);
            if (req->magic != REQ_MAGIC || fnv1a(req->data, req->len) != req->sum)
                die2("service: bad ping parcel");

            uint32_t term = (s.txn.code == CODE_TERM);
            struct rep_parcel *rep = (struct rep_parcel *)(g_map + SCRATCH_OFF2);
            rep->magic = REP_MAGIC;
            rep->val = (s.txn.code == CODE_ECHO) ? (uint32_t)oneways : req->sum;
            rep->sum2 = req->sum ^ 0xA5A5A5A5u;
            memset(rep->data, 0x5A, DATA_LEN);

            uint64_t off0 = 0;
            size_t rdata = sizeof(*rep);
            size_t roffs = 0;
            if (reply_with_fd >= 0) {
                struct flat_binder_object rfo;
                memset(&rfo, 0, sizeof(rfo));
                rfo.type = BINDER_TYPE_FD;
                rfo.handle = (uint32_t)reply_with_fd;
                memcpy(g_map + SCRATCH_OFF, &rfo, sizeof(rfo));
                memcpy(g_map + SCRATCH_OFF + sizeof(rfo), rep, sizeof(*rep));
                rdata = sizeof(rfo) + sizeof(*rep);
                roffs = 8;
            } else {
                memcpy(g_map + SCRATCH_OFF, rep, sizeof(*rep));
            }

            struct binder_transaction_data rt;
            memset(&rt, 0, sizeof(rt));
            rt.code = s.txn.code;
            rt.flags = 0;
            rt.data_size = rdata;
            rt.offsets_size = roffs;
            rt.data.ptr.buffer = (uintptr_t)(g_map + SCRATCH_OFF);
            rt.data.ptr.offsets = (uintptr_t)&off0;

            struct bcbuf r;
            bc_reset(&r);
            bc_u32(&r, BC_REPLY);
            bc_put(&r, &rt, sizeof(rt));
            bc_u32(&r, BC_FREE_BUFFER);
            bc_u64(&r, (uint64_t)(uintptr_t)buf);
            if (bwr_write_only(&r) < 0)
                die("service reply");

            pings++;
            if (term) {
                printf("B2 SERVICE DONE: pings=%d oneways=%d\n", pings, oneways);
                fflush(stdout);
                munmap(g_map, MAP_SIZE);
                close(g_fd); /* death notifications fire here */
                _exit(0);
            }
        }
    }
    return 0;
}

/* ---- hwbinder context manager: minimal echo server ---- */

static int run_hwmgr(int ready_pipe)
{
    g_role = "hwmgr";
    g_dev = "/dev/hwbinder";
    signal(SIGALRM, on_alarm);
    alarm(180);

    binder_setup(1);
    if (write(ready_pipe, "R", 1) != 1)
        die("hwmgr ready pipe");

    int served = 0;
    while (1) {
        struct br_seen s;
        memset(&s, 0, sizeof(s));
        struct bcbuf c;
        bc_reset(&c);
        queue_node_dones(&c);
        if (bwr_round(&c, &s) < 0 || s.fatal)
            die("hwmgr bwr");
        struct binder_transaction_data td;
        while (take_txn(&td)) {
        s.txn = td;
        served++;
        uint8_t *buf = (uint8_t *)(uintptr_t)s.txn.data.ptr.buffer;
        struct req_parcel *req = (struct req_parcel *)buf;
        if (req->magic != REQ_MAGIC)
            die2("hwmgr: bad parcel");

        uint32_t term = (s.txn.code == CODE_TERM);
        struct rep_parcel *rep = (struct rep_parcel *)(g_map + SCRATCH_OFF2);
        rep->magic = REP_MAGIC;
        rep->val = req->sum ^ 0xA1A1A1A1u;
        memset(rep->data, 0xA1, DATA_LEN);
        memcpy(g_map + SCRATCH_OFF, rep, sizeof(*rep));

        uint64_t off0 = 0;
        struct binder_transaction_data rt;
        memset(&rt, 0, sizeof(rt));
        rt.code = s.txn.code;
        rt.flags = 0;
        rt.data_size = sizeof(*rep);
        rt.offsets_size = 0;
        rt.data.ptr.buffer = (uintptr_t)(g_map + SCRATCH_OFF);
        rt.data.ptr.offsets = (uintptr_t)&off0;

        struct bcbuf r;
        bc_reset(&r);
        bc_u32(&r, BC_REPLY);
        bc_put(&r, &rt, sizeof(rt));
        bc_u32(&r, BC_FREE_BUFFER);
        bc_u64(&r, (uint64_t)(uintptr_t)buf);
        if (bwr_write_only(&r) < 0)
            die("hwmgr reply");

        if (term) {
            printf("B2 HWMGR DONE: served=%d\n", served);
            fflush(stdout);
            munmap(g_map, MAP_SIZE);
            close(g_fd);
            _exit(0);
        }
        }
    }
    return 0;
}

/* Context-isolation check (round 0 only): /dev/hwbinder has its own
 * manager; /dev/vndbinder exists; handle 0 on hwbinder is dead until
 * its manager installs; bogus handles stay invalid. */
static int run_hwtest(void)
{
    g_role = "hwtest";
    g_dev = "/dev/hwbinder";
    signal(SIGALRM, on_alarm);
    alarm(180);

    /* vndbinder smoke: node exists and speaks protocol 8 */
    {
        int fd = open("/dev/vndbinder", O_RDWR | O_CLOEXEC);
        if (fd < 0)
            die("open /dev/vndbinder");
        uint8_t *m = mmap(NULL, MAP_SIZE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
        if (m == MAP_FAILED)
            die("mmap vndbinder");
        struct binder_version v;
        v.protocol_version = -1;
        if (ioctl(fd, BINDER_VERSION, &v) < 0 || v.protocol_version != 8)
            die2("vndbinder version");
        munmap(m, MAP_SIZE);
        close(fd);
    }

    binder_setup(0);

    /* handle 0 on hwbinder with no manager installed -> BR_DEAD_REPLY */
    {
        size_t ds = build_plain_req(81);
        struct br_seen s;
        if (do_txn(0, CODE_ECHO, ds, 0, &s) < 0)
            die("hwtest dead mgr txn");
        if (!s.dead_reply)
            die2("hwtest: handle 0 answered with no hwbinder manager");
    }

    /* bring up the hwbinder manager */
    int p[2];
    if (pipe(p) < 0)
        return -1;
    pid_t pid = fork();
    if (pid < 0)
        return -1;
    if (pid == 0) {
        close(p[0]);
        _exit(run_hwmgr(p[1]));
    }
    close(p[1]);
    char r = 0;
    if (read(p[0], &r, 1) != 1 || r != 'R')
        die2("hwtest: hwmgr not ready");

    /* handle 0 now routes to the HW manager */
    {
        size_t ds = build_plain_req(82);
        struct br_seen s;
        if (do_txn(0, CODE_ECHO, ds, 0, &s) < 0)
            die("hwtest echo txn");
        if (!s.have_reply)
            die2("hwtest: no reply from hwbinder manager");
        uint8_t *rbuf = (uint8_t *)(uintptr_t)s.reply.data.ptr.buffer;
        struct rep_parcel *rep = (struct rep_parcel *)rbuf;
        if (rep->magic != REP_MAGIC)
            die2("hwtest: bad hwbinder reply");
        free_buffer((uint64_t)(uintptr_t)rbuf);
    }

    /* bogus handle in THIS open (no refs imported) -> BR_FAILED_REPLY */
    {
        size_t ds = build_plain_req(83);
        struct br_seen s;
        if (do_txn(7, CODE_ECHO, ds, 0, &s) < 0)
            die("hwtest bogus txn");
        if (!s.failed_reply && !s.dead_reply)
            die2("hwtest: bogus handle accepted");
    }

    /* TERM the HW manager */
    {
        size_t ds = build_plain_req(84);
        struct br_seen s;
        if (do_txn(0, CODE_TERM, ds, 0, &s) < 0)
            die("hwtest term txn");
        if (!s.have_reply)
            die2("hwtest: no reply to TERM");
        uint8_t *rbuf = (uint8_t *)(uintptr_t)s.reply.data.ptr.buffer;
        free_buffer((uint64_t)(uintptr_t)rbuf);
    }
    int st;
    if (waitpid(pid, &st, 0) < 0 || !WIFEXITED(st) || WEXITSTATUS(st) != 0)
        die2("hwtest: hwmgr exit status");

    munmap(g_map, MAP_SIZE);
    close(g_fd);
    g_dev = "/dev/binder";
    return 0;
}

/* ---- client: the verifier ---- */

static int run_client(int ready_pipe)
{
    g_role = "client";
    signal(SIGALRM, on_alarm);
    alarm(180);

    binder_setup(0);
    char r = 0;
    if (read(ready_pipe, &r, 1) != 1 || r != 'R')
        die("client ready read");

    int handshakes = 0;

    /* 1. GET_SERVICE -> handle */
    uint32_t svc = 0;
    {
        size_t ds = build_plain_req(11);
        struct br_seen s;
        if (do_txn(0, CODE_GET, ds, 0, &s) < 0)
            die("client get txn");
        if (!s.have_reply)
            die2("client: no reply to GET");
        handshakes += s.node_refs;
        uint8_t *rbuf = (uint8_t *)(uintptr_t)s.reply.data.ptr.buffer;
        if (s.reply.offsets_size != 8)
            die2("client: GET reply has no object");
        struct flat_binder_object *fo = (struct flat_binder_object *)rbuf;
        if (fo->type != BINDER_TYPE_HANDLE)
            die2("client: GET object not a HANDLE");
        svc = fo->handle;
        if (svc == 0)
            die2("client: handle 0 from samgr");
        struct rep_parcel *rep = (struct rep_parcel *)(rbuf + sizeof(*fo));
        if (rep->magic != REP_MAGIC || rep->val != 2)
            die2("client: GET reply bad");
        if (free_buffer((uint64_t)(uintptr_t)rbuf) < 0)
            die("client free");
    }

    /* 2. twoway ping through the service handle */
    {
        size_t ds = build_plain_req(21);
        struct br_seen s;
        if (do_txn(svc, CODE_ECHO, ds, 0, &s) < 0)
            die("client ping txn");
        if (!s.have_reply)
            die2("client: no reply to ping");
        handshakes += s.node_refs;
        uint8_t *rbuf = (uint8_t *)(uintptr_t)s.reply.data.ptr.buffer;
        struct rep_parcel *rep = (struct rep_parcel *)rbuf;
        if (rep->magic != REP_MAGIC)
            die2("client: ping reply bad");
        if (free_buffer((uint64_t)(uintptr_t)rbuf) < 0)
            die("client free ping");
    }

    /* flush handshake dones */
    {
        struct bcbuf c;
        bc_reset(&c);
        queue_node_dones(&c);
        if (c.len && bwr_write_only(&c) < 0)
            die("client dones");
    }

    /* F1. fd passing both ways: send a pipe write end to the service
     * (BINDER_TYPE_FD in the txn); the reply carries one of the service's
     * own fds back. Requires the service node to accept fds and the txn
     * to carry TF_ACCEPT_FDS for the reply fd. */
    {
        int pipeX[2];
        if (pipe(pipeX) < 0)
            die("client pipeX");
        size_t ds = build_plain_req(61);
        /* move the parcel behind the object, then place the fd object */
        memmove(g_map + SCRATCH_OFF + sizeof(struct flat_binder_object),
                g_map + SCRATCH_OFF, sizeof(struct req_parcel));
        struct flat_binder_object fo;
        memset(&fo, 0, sizeof(fo));
        fo.type = BINDER_TYPE_FD;
        fo.handle = (uint32_t)pipeX[1];
        memcpy(g_map + SCRATCH_OFF, &fo, sizeof(fo));

        uint64_t off0 = 0;
        struct binder_transaction_data tr;
        memset(&tr, 0, sizeof(tr));
        tr.target.handle = svc;
        tr.code = CODE_SENDFD;
        tr.flags = 0x10;
        tr.data_size = sizeof(fo) + sizeof(struct req_parcel);
        tr.offsets_size = 8;
        tr.data.ptr.buffer = (uintptr_t)(g_map + SCRATCH_OFF);
        tr.data.ptr.offsets = (uintptr_t)&off0;
        (void)ds;

        struct bcbuf c;
        bc_reset(&c);
        queue_node_dones(&c);
        bc_u32(&c, BC_TRANSACTION);
        bc_put(&c, &tr, sizeof(tr));

        struct br_seen s;
        memset(&s, 0, sizeof(s));
        while (!s.have_reply && !s.dead_reply && !s.failed_reply && !s.fatal) {
            struct br_seen r1;
            memset(&r1, 0, sizeof(r1));
            if (bwr_round(&c, &r1) < 0)
                die("client sendfd bwr");
            bc_reset(&c);
            queue_node_dones(&c);
            if (r1.have_reply) {
                s.reply = r1.reply;
                s.have_reply = 1;
            }
            if (r1.dead_reply || r1.failed_reply) {
                s.dead_reply += r1.dead_reply;
                s.failed_reply += r1.failed_reply;
                break;
            }
        }
        if (!s.have_reply)
            die2("client: no reply to SENDFD");
        uint8_t *rbuf = (uint8_t *)(uintptr_t)s.reply.data.ptr.buffer;
        if (s.reply.offsets_size != 8)
            die2("client: SENDFD reply has no fd object");
        struct flat_binder_object *rfo = (struct flat_binder_object *)rbuf;
        if (rfo->type != BINDER_TYPE_FD)
            die2("client: SENDFD reply object not an FD");
        int svc_fd = (int)rfo->handle;
        if (svc_fd <= 2)
            die2("client: reply fd implausible");
        struct rep_parcel *rep = (struct rep_parcel *)(rbuf + sizeof(*rfo));
        if (rep->magic != REP_MAGIC)
            die2("client: SENDFD reply bad");
        if (free_buffer((uint64_t)(uintptr_t)rbuf) < 0)
            die("client free sendfd");
        /* write through the received fd; the service must observe it */
        if (write(svc_fd, "R", 1) != 1)
            die2("client: write via service fd");
        close(svc_fd);
        /* the service wrote 'S' through OUR pipe write end */
        char b = 0;
        if (read(pipeX[0], &b, 1) != 1 || b != 'S')
            die2("client: no byte from service via translated fd");
        close(pipeX[0]);
        close(pipeX[1]);
    }

    /* F2. sg buffers + fd array: BC_TRANSACTION_SG with a BINDER_TYPE_PTR
     * (extra raw data, copied into the target's sg area) and a
     * BINDER_TYPE_FDA (two fds inside the parent buffer). */
    {
        int pipeA[2], pipeB[2];
        if (pipe(pipeA) < 0 || pipe(pipeB) < 0)
            die("client pipes");
        uint8_t raw[64];
        memset(raw, 0, sizeof(raw));
        memcpy(raw, "RAWDATA", 7);
        memcpy(raw + 24, "TAILDATA", 8);
        uint32_t f0 = (uint32_t)pipeA[1];
        uint32_t f1 = (uint32_t)pipeB[1];
        memcpy(raw + 16, &f0, 4);
        memcpy(raw + 20, &f1, 4);

        /* parcel: [PTR(40)][FDA(32)][req] */
        struct binder_buffer_object bp;
        memset(&bp, 0, sizeof(bp));
        bp.hdr_type = BINDER_TYPE_PTR;
        bp.flags = 0; /* no parent fixup */
        bp.buffer = (uintptr_t)raw;
        bp.length = 64;
        struct binder_fd_array_object fda;
        memset(&fda, 0, sizeof(fda));
        fda.hdr_type = BINDER_TYPE_FDA;
        fda.num_fds = 2;
        fda.parent = 0;
        fda.parent_offset = 16;

        uint8_t *base = g_map + SCRATCH_OFF;
        memcpy(base, &bp, sizeof(bp));
        memcpy(base + sizeof(bp), &fda, sizeof(fda));
        struct req_parcel *req = (struct req_parcel *)(base + sizeof(bp) + sizeof(fda));
        req->magic = REQ_MAGIC;
        req->len = DATA_LEN;
        req->pad = 71;
        for (uint32_t i = 0; i < DATA_LEN; i++)
            req->data[i] = (uint8_t)((71 * 131 + i * 7) & 0xff);
        req->sum = fnv1a(req->data, DATA_LEN);

        struct br_seen s;
        if (do_txn_sg(svc, CODE_SENDRAW,
                      sizeof(bp) + sizeof(fda) + sizeof(*req), 16,
                      64, &s) < 0)
            die("client sendraw txn");
        if (!s.have_reply)
            die2("client: no reply to SENDRAW");
        uint8_t *rbuf = (uint8_t *)(uintptr_t)s.reply.data.ptr.buffer;
        struct rep_parcel *rep = (struct rep_parcel *)rbuf;
        if (rep->magic != REP_MAGIC)
            die2("client: SENDRAW reply bad");
        if (free_buffer((uint64_t)(uintptr_t)rbuf) < 0)
            die("client free sendraw");

        char b1 = 0, b2 = 0;
        if (read(pipeA[0], &b1, 1) != 1 || b1 != '1')
            die2("client: no byte via fda fd0");
        if (read(pipeB[0], &b2, 1) != 1 || b2 != '2')
            die2("client: no byte via fda fd1");
        close(pipeA[0]); close(pipeA[1]);
        close(pipeB[0]); close(pipeB[1]);
    }

    /* F3. oneway parking: three TF_ONE_WAY pings in a row — the node's
     * async slot holds one, the rest must queue (node->async_todo), not
     * fail; a sync probe then waits until all three were served. */
    {
        for (uint32_t k = 0; k < 3; k++) {
            size_t ds = build_plain_req(90 + k);
            uint64_t off0 = 0;
            struct binder_transaction_data tr;
            memset(&tr, 0, sizeof(tr));
            tr.target.handle = svc;
            tr.code = CODE_ECHO;
            tr.flags = TF_ONE_WAY;
            tr.data_size = ds;
            tr.offsets_size = 0;
            tr.data.ptr.buffer = (uintptr_t)(g_map + SCRATCH_OFF);
            tr.data.ptr.offsets = (uintptr_t)&off0;

            struct bcbuf c;
            bc_reset(&c);
            queue_node_dones(&c);
            bc_u32(&c, BC_TRANSACTION);
            bc_put(&c, &tr, sizeof(tr));

            struct br_seen s;
            memset(&s, 0, sizeof(s));
            if (bwr_round(&c, &s) < 0 || s.fatal)
                die("client oneway bwr");
            if (s.failed_reply)
                die2("client: oneway rejected (async slot busy)");
            if (s.complete != 1)
                die2("client: oneway missing BR_TRANSACTION_COMPLETE");
        }
        /* sync probe: ECHO replies with the service's oneway count */
        int served = -1;
        for (int k = 0; k < 20 && served < 3; k++) {
            size_t ds = build_plain_req(93);
            struct br_seen s;
            if (do_txn(svc, CODE_ECHO, ds, 0, &s) < 0)
                die("client chk txn");
            if (!s.have_reply)
                die2("client: no reply to oneway check");
            uint8_t *rbuf = (uint8_t *)(uintptr_t)s.reply.data.ptr.buffer;
            struct rep_parcel *rep = (struct rep_parcel *)rbuf;
            served = (int)rep->val;
            if (free_buffer((uint64_t)(uintptr_t)rbuf) < 0)
                die("client free chk");
        }
        if (served != 3)
            die2("client: parked oneways never fully drained");
    }

    /* 3. request (B) then clear -> CLEAR_DONE(B), no death
     *    (one death request per handle: clear path first) */
    {
        struct bcbuf c;
        bc_reset(&c);
        bc_u32(&c, BC_REQUEST_DEATH_NOTIFICATION);
        bc_u32(&c, svc);
        bc_u64(&c, COOKIE_B);
        bc_u32(&c, BC_CLEAR_DEATH_NOTIFICATION);
        bc_u32(&c, svc);
        bc_u64(&c, COOKIE_B);
        if (bwr_write_only(&c) < 0)
            die("client req+clear B");
        if (wait_clear_done(COOKIE_B) < 0)
            die("client clear-done wait B");
        if (g_dead_seen_n != 0)
            die2("client: spurious death on clear");
    }

    /* 4. request death (A) — the live watch */
    {
        struct bcbuf c;
        bc_reset(&c);
        bc_u32(&c, BC_REQUEST_DEATH_NOTIFICATION);
        bc_u32(&c, svc);
        bc_u64(&c, COOKIE_A);
        if (bwr_write_only(&c) < 0)
            die("client req death A");
    }

    /* 5. TERM the service; BR_DEAD_BINDER(COOKIE_A) must arrive (the
     *    reply and the death may share one read round — handled). */
    {
        size_t ds = build_plain_req(31);
        struct br_seen s;
        if (do_txn(svc, CODE_TERM, ds, 0, &s) < 0)
            die("client term txn");
        if (!s.have_reply)
            die2("client: no reply to TERM");
        uint8_t *rbuf = (uint8_t *)(uintptr_t)s.reply.data.ptr.buffer;
        free_buffer((uint64_t)(uintptr_t)rbuf);
        if (take_death(COOKIE_B))
            die2("client: cleared watch B still fired");

        if (wait_death(COOKIE_A) < 0)
            die("client death wait A");
        if (g_dead_seen_n != 0)
            die2("client: unexpected extra death cookies");
    }

    /* 6. txn to the dead handle -> BR_DEAD_REPLY */
    {
        size_t ds = build_plain_req(41);
        struct br_seen s;
        if (do_txn(svc, CODE_ECHO, ds, 0, &s) < 0)
            die("client dead txn");
        if (!s.dead_reply)
            die2("client: expected BR_DEAD_REPLY to dead handle");
        if (s.have_reply)
            die2("client: dead handle still answers");
    }

    /* 7. clear the fired watch (A) -> CLEAR_DONE(A), then request death
     *    on the already-dead handle (C) -> immediate BR_DEAD_BINDER */
    {
        struct bcbuf c;
        bc_reset(&c);
        bc_u32(&c, BC_CLEAR_DEATH_NOTIFICATION);
        bc_u32(&c, svc);
        bc_u64(&c, COOKIE_A);
        if (bwr_write_only(&c) < 0)
            die("client clear A");
        if (wait_clear_done(COOKIE_A) < 0)
            die("client clear-done wait A");

        bc_reset(&c);
        bc_u32(&c, BC_REQUEST_DEATH_NOTIFICATION);
        bc_u32(&c, svc);
        bc_u64(&c, COOKIE_C);
        if (bwr_write_only(&c) < 0)
            die("client req death C");
        if (wait_death(COOKIE_C) < 0)
            die("client death wait C");
    }

    /* 8. release our refs so the world settles before samgr TERM */
    {
        struct bcbuf c;
        bc_reset(&c);
        bc_u32(&c, BC_RELEASE);
        bc_u32(&c, svc);
        if (bwr_write_only(&c) < 0)
            die("client release");
    }

    /* 9. TERM samgr */
    {
        size_t ds = build_plain_req(51);
        struct br_seen s;
        if (do_txn(0, CODE_TERM, ds, 0, &s) < 0)
            die("client term samgr");
        if (!s.have_reply)
            die2("client: no reply to samgr TERM");
        uint8_t *rbuf = (uint8_t *)(uintptr_t)s.reply.data.ptr.buffer;
        free_buffer((uint64_t)(uintptr_t)rbuf);
    }

    printf("B2 CLIENT DONE: handle=%u handshakes=%d\n", svc, handshakes);
    fflush(stdout);
    munmap(g_map, MAP_SIZE);
    close(g_fd);
    return 0;
}

#define ROUNDS 3

int main(void)
{
    setvbuf(stdout, NULL, _IONBF, 0);

    long mem0 = read_memfree();
    for (int round = 0; round < ROUNDS; round++) {
        /* 1. samgr starts first and becomes the context manager */
        int p2a[2];
        if (pipe(p2a) < 0) {
            printf("B2 PROBE RESULT: FAIL pipe errno=%d\n", errno);
            return 2;
        }
        pid_t samgr_pid = fork();
        if (samgr_pid < 0) {
            printf("B2 PROBE RESULT: FAIL fork errno=%d\n", errno);
            return 2;
        }
        if (samgr_pid == 0) {
            close(p2a[0]);
            _exit(run_samgr(p2a[1]));
        }
        close(p2a[1]);
        char r = 0;
        if (read(p2a[0], &r, 1) != 1 || r != 'R') {
            printf("B2 PROBE RESULT: FAIL samgr not ready\n");
            return 2;
        }

        /* 2. service registers once samgr is up */
        int s_go[2], s_ok[2];
        if (pipe(s_go) < 0 || pipe(s_ok) < 0)
            return 2;
        pid_t svc_pid = fork();
        if (svc_pid < 0) {
            printf("B2 PROBE RESULT: FAIL fork errno=%d\n", errno);
            return 2;
        }
        if (svc_pid == 0) {
            close(p2a[0]);
            close(s_go[1]);
            close(s_ok[0]);
            run_service(s_go[0], s_ok[1]);
            _exit(0);
        }
        close(s_go[0]);
        close(s_ok[1]);
        if (write(s_go[1], "R", 1) != 1)
            return 2;
        if (read(s_ok[0], &r, 1) != 1 || r != 'S') {
            printf("B2 PROBE RESULT: FAIL service not registered\n");
            return 2;
        }
        close(s_ok[0]);

        /* 3. client drives the full lifecycle */
        int c_go[2];
        if (pipe(c_go) < 0)
            return 2;
        pid_t cli_pid = fork();
        if (cli_pid < 0) {
            printf("B2 PROBE RESULT: FAIL fork errno=%d\n", errno);
            return 2;
        }
        if (cli_pid == 0) {
            close(p2a[0]);
            close(s_go[1]);
            close(c_go[1]);
            _exit(run_client(c_go[0]));
        }
        close(c_go[0]);
        close(s_go[1]);
        if (write(c_go[1], "R", 1) != 1)
            return 2;
        close(c_go[1]);

        int cli_st = 0, svc_st = 0, mgr_st = 0;
        if (waitpid(cli_pid, &cli_st, 0) < 0)
            return 2;
        waitpid(svc_pid, &svc_st, 0);
        waitpid(samgr_pid, &mgr_st, 0);
        close(p2a[0]);

        if (!WIFEXITED(cli_st) || WEXITSTATUS(cli_st) != 0) {
            printf("B2 PROBE RESULT: FAIL client status %d\n", cli_st);
            return 1;
        }
        if (!WIFEXITED(svc_st) || WEXITSTATUS(svc_st) != 0) {
            printf("B2 PROBE RESULT: FAIL service status %d\n", svc_st);
            return 1;
        }
        if (!WIFEXITED(mgr_st) || WEXITSTATUS(mgr_st) != 0) {
            printf("B2 PROBE RESULT: FAIL samgr status %d (deaths!=1)\n", mgr_st);
            return 1;
        }

        if (round == 0) {
            if (run_hwtest() != 0)
                die("hwtest");
            printf("B2 HWTEST DONE\n");
            fflush(stdout);
        }

        long mf = read_memfree();
        printf("B2 PROBE ROUND %d/%d memfree=%ld kB\n", round + 1, ROUNDS, mf);
        if (mem0 >= 0 && mf >= 0 && round >= 1) {
            static long prev = -1;
            if (prev >= 0 && prev - mf > 2048) {
                printf("B2 PROBE RESULT: FAIL memory drift %ld -> %ld kB\n", prev, mf);
                return 1;
            }
            prev = mf;
            if (round == ROUNDS - 1)
                printf("B2 PROBE RESULT: PASS rounds=%d memfree=%ld kB\n", ROUNDS, mf);
        }
    }
    return 0;
}
