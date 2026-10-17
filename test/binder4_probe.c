/* binder4_probe.c — binder S4: from-walk + wake-eligibility probe.
 *
 * Validates the two kernel fixes for the b85 samgr wedge, on the
 * cross-process path (mechanically identical to same-proc delivery):
 *
 *  A. from-walk (Linux 5.10 binder_transaction target_thread selection):
 *     P (context manager, ONE worker thread) handles Q's REG transaction
 *     and, before replying, makes a nested SYNC call back to the handle Q
 *     just registered. Q's only thread is parked in read waiting for the
 *     REG reply — without thread-targeted delivery the nested txn lands
 *     on Q's proc todo where no eligible Q reader exists (the single
 *     thread has a stacked txn) and the pair deadlocks A<->B.
 *
 *  B. wake eligibility (waiting_threads parity): P parks a reader thread
 *     that NEVER issues BC_ENTER_LOOPER before its blocking read. With
 *     the old wake_proc_one (no looper check) that thread is first in
 *     the proc's thread list and eats every wake — Q's subsequent sync
 *     PINGs to the manager are never served. With the looper gate the
 *     flagless reader is skipped and the ENTERED worker answers.
 *
 * Topology (x86_64, host gcc -static):
 *
 *   P (parent)                          Q (child, fresh open)
 *   ----------------------              ----------------------
 *   open /dev/binder + mmap             close inherited fd, fresh open
 *   SET_MAX_THREADS(8)                  ENTER_LOOPER
 *   SET_CONTEXT_MGR                     REG (handle 0, BINDER obj)
 *   [dummy thread: read, no looper]       <- parked waiting for reply
 *   ENTER_LOOPER (main = worker)        receives nested ECHO on its OWN
 *   on REG: save handle, send nested      thread todo, replies, then
 *   SYNC ECHO to Q (from-walk),          gets the REG reply
 *   reply REG after echo reply
 *   loop: PING -> reply ping             20 PING rounds (wake test)
 *
 * Output contract:
 *   B4 PROBE RESULT: PASS pings=20   (exit 0)
 *   B4 PROBE RESULT: FAIL ...        (exit 1)
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
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
    BINDER_TYPE_HANDLE = B_PACK_CHARS('s', 'h', '*', B_TYPE_LARGE),
};

#define BINDER_WRITE_READ _IOWR('b', 1, struct binder_write_read)
#define BINDER_SET_MAX_THREADS _IOW('b', 5, uint32_t)
#define BINDER_SET_CONTEXT_MGR _IOW('b', 7, int32_t)

#define BC_TRANSACTION _IOW('c', 0, struct binder_transaction_data)
#define BC_REPLY _IOW('c', 1, struct binder_transaction_data)
#define BC_FREE_BUFFER _IOW('c', 3, binder_uintptr_t)
#define BC_ACQUIRE_DONE _IOW('c', 9, struct binder_ptr_cookie)
#define BC_INCREFS_DONE _IOW('c', 8, struct binder_ptr_cookie)
#define BC_ENTER_LOOPER _IO('c', 12)

#define BR_ERROR _IOR('r', 0, int32_t)
#define BR_TRANSACTION _IOR('r', 2, struct binder_transaction_data)
#define BR_REPLY _IOR('r', 3, struct binder_transaction_data)
#define BR_DEAD_REPLY _IO('r', 5)
#define BR_TRANSACTION_COMPLETE _IO('r', 6)
#define BR_INCREFS _IOR('r', 7, struct binder_ptr_cookie)
#define BR_ACQUIRE _IOR('r', 8, struct binder_ptr_cookie)
#define BR_RELEASE _IOR('r', 9, struct binder_ptr_cookie)
#define BR_DECREFS _IOR('r', 10, struct binder_ptr_cookie)
#define BR_NOOP _IO('r', 12)
#define BR_SPAWN_LOOPER _IO('r', 13)
#define BR_FAILED_REPLY _IO('r', 17)

#define TF_ONE_WAY 0x01

#define CODE_REG 1
#define CODE_ECHO 2
#define CODE_PING 3

#define OBJ_PTR 0xAAAAUL
#define OBJ_COOKIE 0x1234UL
#define MAGIC 0x515152585558ULL
#define PING_ROUNDS 20

static int bfd = -1;

static void die(const char *msg)
{
    fprintf(stderr, "b4: FATAL %s: %s\n", msg, strerror(errno));
    exit(3);
}

static void raw_write(void *buf, size_t n)
{
    struct binder_write_read bwr;
    memset(&bwr, 0, sizeof(bwr));
    bwr.write_size = n;
    bwr.write_buffer = (uintptr_t)buf;
    if (ioctl(bfd, BINDER_WRITE_READ, &bwr) < 0)
        die("raw_write");
}

static void wr_cmd(uint32_t cmd)
{
    uint32_t c = cmd;
    raw_write(&c, 4);
}

static void node_done(uint32_t cmd, uint64_t ptr, uint64_t cookie)
{
    struct binder_ptr_cookie pc = { ptr, cookie };
    char b[32];
    memcpy(b, &cmd, 4);
    memcpy(b + 4, &pc, 16);
    raw_write(b, 20);
}

static void free_buffer(uint64_t ptr)
{
    char b[16];
    uint32_t cmd = BC_FREE_BUFFER;
    memcpy(b, &cmd, 4);
    memcpy(b + 4, &ptr, 8);
    raw_write(b, 12);
}

/* One transaction write. handle < 0 => BC_REPLY. Payload layout when
 * with_obj: [magic:8][flat_binder_object:24], object offset 8. The
 * stream must be BYTE-PACKED ([cmd:4][tr:64]...) — a struct with
 * cmd + tr would insert alignment padding before tr. */
static void send_txn(int handle, uint32_t code, uint64_t magic,
                     int with_obj)
{
    struct binder_transaction_data tr;
    uint64_t payload[8]; /* magic + flat object */
    uint64_t offsets[1];
    memset(&tr, 0, sizeof(tr));
    tr.target.handle = handle < 0 ? 0 : (uint32_t)handle;
    tr.code = code;
    tr.flags = 0x10; /* TF_ACCEPT_FDS */
    tr.data.ptr.buffer = (uintptr_t)payload;
    payload[0] = magic;
    if (with_obj) {
        struct flat_binder_object obj;
        memset(&obj, 0, sizeof(obj));
        obj.type = BINDER_TYPE_BINDER;
        obj.binder = OBJ_PTR;
        obj.cookie = OBJ_COOKIE;
        memcpy(&payload[1], &obj, sizeof(obj));
        offsets[0] = 8;
        tr.data_size = 8 + sizeof(obj);
        tr.offsets_size = 8;
        tr.data.ptr.offsets = (uintptr_t)offsets;
    } else {
        tr.data_size = 8;
    }
    char b[8 + sizeof(tr)];
    uint32_t cmd = handle < 0 ? BC_REPLY : BC_TRANSACTION;
    memcpy(b, &cmd, 4);
    memcpy(b + 4, &tr, sizeof(tr));
    raw_write(b, 4 + sizeof(tr));
}

struct br_ev {
    uint32_t cmd;
    struct binder_transaction_data tr;
    struct binder_ptr_cookie pc;
};

static int read_cmds(struct br_ev *evs, int max)
{
    static char rb[1024];
    struct binder_write_read bwr;
    memset(&bwr, 0, sizeof(bwr));
    bwr.read_size = sizeof(rb);
    bwr.read_buffer = (uintptr_t)rb;
    if (ioctl(bfd, BINDER_WRITE_READ, &bwr) < 0)
        die("read");
    int n = 0;
    uint8_t *p = (uint8_t *)rb;
    uint8_t *end = p + bwr.read_consumed;
    while (p + 4 <= end && n < max) {
        memcpy(&evs[n].cmd, p, 4);
        p += 4;
        if (evs[n].cmd == BR_TRANSACTION || evs[n].cmd == BR_REPLY) {
            memcpy(&evs[n].tr, p, 64);
            p += 64;
        } else if (evs[n].cmd == BR_INCREFS || evs[n].cmd == BR_ACQUIRE ||
                   evs[n].cmd == BR_RELEASE || evs[n].cmd == BR_DECREFS) {
            memcpy(&evs[n].pc, p, 16);
            p += 16;
        }
        n++;
    }
    return n;
}

static uint64_t magic_of(struct binder_transaction_data *tr)
{
    uint64_t *d = (uint64_t *)(uintptr_t)tr->data.ptr.buffer;
    return d ? *d : 0;
}

/* ------------------------------------------------------------------ */
/* P: the context manager                                              */

static uint32_t q_handle;

/* The dummy thread ALSO performs the setup ioctls (max threads, context
 * manager) so its kernel thread entry is FIRST in the proc's list —
 * with the old wake_proc_one that made it eat every wake for the pool. */
static void *black_hole(void *arg)
{
    (void)arg;
    uint32_t max = 8;
    if (ioctl(bfd, BINDER_SET_MAX_THREADS, &max) < 0)
        die("SET_MAX_THREADS");
    if (ioctl(bfd, BINDER_SET_CONTEXT_MGR, &(int32_t){ 0 }) < 0)
        die("SET_CONTEXT_MGR");
    struct br_ev evs[4];
    for (;;)
        read_cmds(evs, 4); /* parks forever; never entered the looper */
    return NULL;
}

static int p_run(int *pings_out)
{
    wr_cmd(BC_ENTER_LOOPER);
    struct br_ev evs[8];
    int reg_state = 0, pings = 0;
    for (;;) {
        int n = read_cmds(evs, 8);
        for (int i = 0; i < n; i++) {
            struct br_ev *e = &evs[i];
            switch (e->cmd) {
            case BR_TRANSACTION: {
                uint64_t magic = magic_of(&e->tr);
                if (e->tr.code == CODE_REG && reg_state == 0) {
                    struct flat_binder_object *o =
                        (struct flat_binder_object *)
                            ((uintptr_t)e->tr.data.ptr.buffer + 8);
                    if (o->type != BINDER_TYPE_HANDLE) {
                        printf("b4: P REG obj type %x (want HANDLE)\n", o->type);
                        return 0;
                    }
                    q_handle = o->handle;
                    printf("b4: P got REG handle %u\n", q_handle); fflush(stdout);
                    printf("b4: P sent nested ECHO\n"); fflush(stdout);
                    reg_state = 1; /* nested echo in flight */
                    send_txn((int)q_handle, CODE_ECHO, magic, 0);
                } else {
                    /* ECHO or anything else: plain reply */
                    send_txn(-1, e->tr.code, magic + 1, 0);
                }
                if (e->tr.code == CODE_PING) {
                    pings++;
                    if (pings == PING_ROUNDS) {
                        *pings_out = pings;
                        return reg_state == 2;
                    }
                }
                free_buffer(e->tr.data.ptr.buffer);
                break;
            }
            case BR_REPLY: {
                uint64_t magic = magic_of(&e->tr);
                if (reg_state == 1) {
                    /* reply to the nested ECHO: finish the REG reply */
                    if (magic != MAGIC + 1) {
                        printf("b4: P echo reply magic %lx\n",
                               (unsigned long)magic);
                        return 0;
                    }
                    send_txn(-1, CODE_REG, magic, 0);
                    reg_state = 2;
                }
                free_buffer(e->tr.data.ptr.buffer);
                break;
            }
            case BR_TRANSACTION_COMPLETE:
            case BR_NOOP:
            case BR_SPAWN_LOOPER:
                break;
            case BR_INCREFS:
                node_done(BC_INCREFS_DONE, e->pc.ptr, e->pc.cookie);
                break;
            case BR_ACQUIRE:
                node_done(BC_ACQUIRE_DONE, e->pc.ptr, e->pc.cookie);
                break;
            case BR_RELEASE:
            case BR_DECREFS:
                break;
            default:
                printf("b4: P unexpected BR %x\n", e->cmd);
                return 0;
            }
        }
    }
}

/* ------------------------------------------------------------------ */
/* Q: registers its object, answers nested ECHO, pings the manager     */

static int q_run(void)
{
    int fd2 = open("/dev/binder", O_RDWR);
    if (fd2 < 0)
        die("Q open");
    close(bfd);
    bfd = fd2;
    if (mmap(NULL, 128 * 1024, PROT_READ, MAP_SHARED, bfd, 0) == MAP_FAILED)
        die("Q mmap");

    wr_cmd(BC_ENTER_LOOPER);
    send_txn(0, CODE_REG, MAGIC, 1);

    struct br_ev evs[8];
    int reg_done = 0, ping_replies = 0;
    for (;;) {
        int n = read_cmds(evs, 8);
        for (int i = 0; i < n; i++) {
            struct br_ev *e = &evs[i];
            switch (e->cmd) {
            case BR_TRANSACTION:
                printf("b4: Q got txn code %u\n", e->tr.code); fflush(stdout);
                send_txn(-1, e->tr.code, magic_of(&e->tr) + 1, 0);
                free_buffer(e->tr.data.ptr.buffer);
                break;
            case BR_REPLY:
                if (!reg_done) {
                    if (magic_of(&e->tr) != MAGIC + 1) {
                        printf("b4: Q REG reply magic %lx\n",
                               (unsigned long)magic_of(&e->tr));
                        return -1;
                    }
                    reg_done = 1;
                } else {
                    ping_replies++;
                }
                free_buffer(e->tr.data.ptr.buffer);
                if (ping_replies == PING_ROUNDS)
                    return 0;
                break;
            case BR_TRANSACTION_COMPLETE:
            case BR_NOOP:
            case BR_SPAWN_LOOPER:
                break;
            case BR_INCREFS:
                node_done(BC_INCREFS_DONE, e->pc.ptr, e->pc.cookie);
                break;
            case BR_ACQUIRE:
                node_done(BC_ACQUIRE_DONE, e->pc.ptr, e->pc.cookie);
                break;
            case BR_RELEASE:
            case BR_DECREFS:
                break;
            default:
                printf("b4: Q unexpected BR %x\n", e->cmd);
                return -1;
            }
        }
        if (reg_done && ping_replies < PING_ROUNDS)
            send_txn(0, CODE_PING, MAGIC + ping_replies, 0);
    }
}

int main(void)
{
    bfd = open("/dev/binder", O_RDWR);
    if (bfd < 0)
        die("open");
    if (mmap(NULL, 128 * 1024, PROT_READ, MAP_SHARED, bfd, 0) == MAP_FAILED)
        die("mmap");

    /* setup ioctls moved into the black-hole thread (thread #1) */

    {
        pthread_t th;
        pthread_create(&th, NULL, black_hole, NULL);
        usleep(100000); /* let it install the manager and park first */
    }

    pid_t q = fork();
    if (q == 0)
        _exit(q_run() == 0 ? 0 : 1);

    int pings = 0;
    int ok = p_run(&pings);
    int st = 0;
    waitpid(q, &st, 0);
    int qok = WIFEXITED(st) && WEXITSTATUS(st) == 0;

    if (ok && qok && pings == PING_ROUNDS)
        printf("B4 PROBE RESULT: PASS pings=%d\n", pings);
    else
        printf("B4 PROBE RESULT: FAIL p=%d q=%d pings=%d\n", ok, qok, pings);
    return 0;
}
