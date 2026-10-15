/* page_scan.S — tiny raw-syscall page checksummer (no libc, one text page).
 *
 * Usage: page_scan <file> [dump_page]
 * Prints "P<idx> <fnv1a-16hex>" for every 4096-byte page of <file>.
 * If dump_page is given, also prints 8 rows "Q<row>:<64 hex bytes>" sampled
 * from that page (row r covers offset r*512 .. r*512+16).
 *
 * Register discipline: s0=fd s1=pageidx s2=n s3=row s4=bufbase s5=dumppage
 * utoa/utohex clobber t0-t6,a1; a0 is cursor in/out. fd/hash live in s-regs.
 *
 * riscv64 syscall numbers: exit=93 write=64 openat=56 read=63.
 */

    .option norvc
    .text
    .globl _start
_start:
    .option push
    .option norelax
    la   gp, __global_pointer$
    .option pop

    lw   t0, 0(sp)               /* argc */
    addi t1, sp, 8               /* argv */
    li   s0, -1                  /* fd = -1 (fail path) */
    li   s5, -1                  /* dump_page disabled */
    li   a0, 1
    blt  t0, a0, 3f              /* need >= 1 arg */
    li   a0, -100                /* AT_FDCWD */
    ld   a1, 8(t1)               /* argv[1] path */
    li   a2, 0                   /* O_RDONLY */
    li   a3, 0
    li   a7, 56                  /* openat */
    ecall
    bltz a0, 3f
    mv   s0, a0                  /* fd */

    lw   t0, 0(sp)               /* argc */
    li   a0, 3
    blt  t0, a0, 1f              /* argv[2] only when argc >= 3 */
    ld   a0, 16(t1)
    jal  ra, atoi
    mv   s5, a0
1:
    li   s1, 0                   /* page index */
loop:
    la   a1, buf
    li   a2, 4096
    mv   a0, s0
    li   a7, 63                  /* read */
    ecall
    bltz a0, 3f
    beqz a0, 2f                  /* EOF */
    mv   s2, a0                  /* n */

    /* fnv1a over buf[0..n) -> s6 */
    la   a3, buf
    mv   a4, s2
    li   s6, 0xcbf29ce484222325
    li   s7, 0x100000001b3
h_loop:
    beqz a4, h_done
    lbu  t6, 0(a3)
    xor  s6, s6, t6
    mul  s6, s6, s7
    addi a3, a3, 1
    addi a4, a4, -1
    j    h_loop
h_done:
    /* print "P<idx> <hash>\n" */
    la   a0, pbuf
    li   t6, 'P'
    sb   t6, 0(a0)
    addi a0, a0, 1
    mv   a1, s1
    jal  ra, utoa
    li   t6, ' '
    sb   t6, 0(a0)
    addi a0, a0, 1
    mv   a1, s6
    jal  ra, utohex
    li   t6, '\n'
    sb   t6, 0(a0)
    addi a0, a0, 1
    la   a1, pbuf
    sub  a2, a0, a1
    li   a0, 1
    li   a7, 64                  /* write */
    ecall

    beq  s1, s5, dump_page
next:
    addi s1, s1, 1
    li   t6, 4096
    beq  s2, t6, loop
2:  li   a0, 0
    li   a7, 93
    ecall
3:  li   a0, 1                   /* stderr-ish path: write ERR, exit 1 */
    la   a1, errmsg
    li   a2, 4
    li   a7, 64
    ecall
    li   a0, 1
    li   a7, 93
    ecall

dump_page:
    li   s3, 0                   /* row */
    la   s4, buf
d_row:
    la   a0, pbuf
    li   t6, 'Q'
    sb   t6, 0(a0)
    addi a0, a0, 1
    mv   a1, s3
    jal  ra, utoa
    li   t6, ':'
    sb   t6, 0(a0)
    addi a0, a0, 1
    li   t6, ' '
    sb   t6, 0(a0)
    addi a0, a0, 1
    /* t1 = buf + row*512 ; print 16 bytes as hex */
    slli t1, s3, 9
    add  t1, t1, s4
    li   t2, 0
    li   s7, 16                  /* reuse s7 (hash prime no longer needed) */
d_byte:
    lbu  t6, 0(t1)
    srli t3, t6, 4
    andi t3, t3, 0xf
    li   t5, 10
    blt  t3, t5, 4f
    addi t3, t3, 'a'-10
    j    5f
4:  addi t3, t3, '0'
5:  sb   t3, 0(a0)
    addi a0, a0, 1
    andi t3, t6, 0xf
    blt  t3, t5, 6f
    addi t3, t3, 'a'-10
    j    7f
6:  addi t3, t3, '0'
7:  sb   t3, 0(a0)
    addi a0, a0, 1
    addi t1, t1, 1
    addi t2, t2, 1
    blt  t2, s7, d_byte
    li   t6, '\n'
    sb   t6, 0(a0)
    addi a0, a0, 1
    la   a1, pbuf
    sub  a2, a0, a1
    li   a0, 1
    li   a7, 64
    ecall
    addi s3, s3, 1
    li   t6, 8
    blt  s3, t6, d_row
    j    next

/* atoi: a0=str -> a0 (positive); clobbers t0-t2 */
atoi:
    li   a1, 0
a_loop:
    lbu  t0, 0(a0)
    beqz t0, a_done
    addi t0, t0, -'0'
    bltz t0, a_done
    li   t1, 9
    bgt  t0, t1, a_done
    li   t2, 10
    mul  a1, a1, t2
    add  a1, a1, t0
    addi a0, a0, 1
    j    a_loop
a_done:
    mv   a0, a1
    ret

/* utoa: a1=value a0=cursor -> a0=end; clobbers t0-t4 */
utoa:
    addi sp, sp, -48
    mv   t0, a0
    mv   t1, sp
    mv   t2, a1
    beqz t2, u_zero
u_loop:
    li   t3, 10
    remu t4, t2, t3
    addi t4, t4, '0'
    sb   t4, 0(t1)
    addi t1, t1, 1
    divu t2, t2, t3
    bnez t2, u_loop
    j    u_copy
u_zero:
    li   t4, '0'
    sb   t4, 0(t1)
    addi t1, t1, 1
u_copy:
    addi t1, t1, -1
    lbu  t4, 0(t1)
    sb   t4, 0(t0)
    addi t0, t0, 1
    bne  t1, sp, u_copy
    mv   a0, t0
    addi sp, sp, 48
    ret

/* utohex: a1=value a0=cursor -> a0=end (16 digits); clobbers t0-t5 */
utohex:
    mv   t0, a0
    li   t1, 60
    li   t5, 10
x_loop:
    srl  t2, a1, t1
    andi t2, t2, 0xf
    blt  t2, t5, x1
    addi t2, t2, 'a'-10
    j    x2
x1: addi t2, t2, '0'
x2: sb   t2, 0(t0)
    addi t0, t0, 1
    addi t1, t1, -4
    bgez t1, x_loop
    mv   a0, t0
    ret

    .data
    .balign 4096
buf:
    .space 4096
    .balign 64
pbuf:
    .space 256
errmsg:
    .ascii "ERR\n"
