/*
 * Karotz syscall probe. Runs as /init (PID 1) in an emulated Linux 2.6.28,
 * or under qemu-arm user mode. No libc: raw ARM EABI syscalls only, so it
 * works on any kernel and pulls in no library code built for newer CPUs.
 *
 * Every call uses deliberately invalid arguments. ENOSYS (-38) means the
 * kernel has no such syscall; any other error means the syscall exists.
 *
 * Build: arm-linux-gnueabi-gcc -marm -march=armv4t -mfloat-abi=soft -Os
 *        -ffreestanding -fno-builtin -fno-stack-protector -nostdlib -static
 *        -o probe probe.c
 */

typedef unsigned long ul;

static long sc(long n, long a, long b, long c, long d, long e, long f)
{
    register long r0 __asm__("r0") = a;
    register long r1 __asm__("r1") = b;
    register long r2 __asm__("r2") = c;
    register long r3 __asm__("r3") = d;
    register long r4 __asm__("r4") = e;
    register long r5 __asm__("r5") = f;
    register long r7 __asm__("r7") = n;
    __asm__ volatile("swi 0"
                     : "+r"(r0)
                     : "r"(r1), "r"(r2), "r"(r3), "r"(r4), "r"(r5), "r"(r7)
                     : "memory");
    return r0;
}

#define NR_exit 1
#define NR_write 4
#define NR_open 5
#define NR_mknod 14
#define NR_getpid 20
#define NR_reboot 88
#define NR_uname 122

static int out = 1;

static ul slen(const char *s) { ul n = 0; while (s[n]) n++; return n; }
static void puts_(const char *s) { sc(NR_write, out, (long)s, slen(s), 0, 0, 0); }

/* Decimal without division (libgcc's divide helper may not be ARMv4T-safe). */
static void putnum(long v)
{
    static const long p10[] = {1000000000, 100000000, 10000000, 1000000,
                               100000, 10000, 1000, 100, 10, 1};
    char buf[16];
    int i, n = 0, started = 0;
    if (v < 0) { buf[n++] = '-'; v = -v; }
    for (i = 0; i < 10; i++) {
        int d = 0;
        while (v >= p10[i]) { v -= p10[i]; d++; }
        if (d || started || i == 9) { buf[n++] = (char)('0' + d); started = 1; }
    }
    buf[n] = 0;
    puts_(buf);
}

struct probe { const char *name; long nr; long a, b, c, d, e, f; const char *need; };

int main(void)
{
    static char uts[6 * 65];
    static char rnd[4];
    /* Arguments chosen to fail fast without side effects.
       static const so gcc doesn't emit a memcpy call to build it. */
    static const struct probe p[] = {
        {"accept",        285, -1, 0, 0, 0, 0, 0,          "std::net (fallback)"},
        {"accept4",       366, -1, 0, 0, 0x80000, 0, 0,    "std::net TcpListener::accept"},
        {"pipe2",         359, 0, 0x7fffffff, 0, 0, 0, 0,  "std::process, mio"},
        {"dup3",          358, -1, -1, 0, 0, 0, 0,         "std::process"},
        {"epoll_create1", 357, 0x7fffffff, 0, 0, 0, 0, 0,  "mio/tokio"},
        {"epoll_pwait",   346, -1, 0, 0, 0, 0, 0,          "mio/tokio"},
        {"eventfd2",      356, 0, 0x7fffffff, 0, 0, 0, 0,  "mio waker"},
        {"signalfd4",     355, -1, 0, 8, 0x7fffffff, 0, 0, "signal crates"},
        {"timerfd_create",350, 99, 0, 0, 0, 0, 0,          "timer crates"},
        {"inotify_init1", 360, 0x7fffffff, 0, 0, 0, 0, 0,  "file watching"},
        {"futex",         240, 0, 1, 1, 0, 0, 0,           "std::sync, threads"},
        {"clock_gettime", 263, 0, 0, 0, 0, 0, 0,           "std::time"},
        {"openat",        322, -1, 0, 0, 0, 0, 0,          "std::fs"},
        {"utimensat",     348, -1, 0, 0, 0, 0, 0,          "std::fs set_times"},
        {"fallocate",     352, -1, 0, 0, 0, 0, 0,          "rarely used"},
        {"preadv",        361, -1, 0, 0, 0, 0, 0,          "std::fs read_vectored_at"},
        {"recvmmsg",      365, -1, 0, 0, 0, 0, 0,          "rarely used"},
        {"prlimit64",     369, 0, 999, 0, 0, 0, 0,         "std (stack size query)"},
        {"syncfs",        373, -1, 0, 0, 0, 0, 0,          "rarely used"},
        {"sendmmsg",      374, -1, 0, 0, 0, 0, 0,          "rarely used"},
        {"getrandom",     384, (long)rnd, 0, 0, 0, 0, 0,   "std HashMap seed (falls back)"},
        {"statx",         397, -100, 0, 0, 0, 0, 0,        "std::fs::metadata (falls back)"},
    };
    unsigned i;
    unsigned long kuser;

    /* As PID 1 in a bare initramfs there may be no console yet.
       Only then: never touch /dev on a real system. */
    if (sc(NR_getpid, 0, 0, 0, 0, 0, 0) == 1) {
        long fd;
        sc(NR_mknod, (long)"/dev/console", 020600, (5 << 8) | 1, 0, 0, 0);
        fd = sc(NR_open, (long)"/dev/console", 2, 0, 0, 0, 0);
        if (fd >= 0) out = (int)fd;
    }

    puts_("\n==== KAROTZ PROBE BEGIN ====\n");
    if (sc(NR_uname, (long)uts, 0, 0, 0, 0, 0) == 0) {
        puts_("uname: "); puts_(uts); puts_(" "); puts_(uts + 2 * 65);
        puts_(" "); puts_(uts + 4 * 65); puts_("\n");
    }

    /* ARM kernel user helpers (used for atomics on ARMv4T/v5). */
    kuser = *(volatile unsigned long *)0xffff0ffc;
    puts_("kuser_helper_version: "); putnum((long)kuser); puts_("\n");

    for (i = 0; i < sizeof(p) / sizeof(p[0]); i++) {
        long r = sc(p[i].nr, p[i].a, p[i].b, p[i].c, p[i].d, p[i].e, p[i].f);
        puts_(r == -38 ? "MISSING  " : "present  ");
        puts_(p[i].name);
        puts_("  (ret "); putnum(r); puts_(")  used by: ");
        puts_(p[i].need); puts_("\n");
    }
    puts_("==== KAROTZ PROBE END ====\n");
    return 0;
}

void _start(void)
{
    main();
    /* Only as PID 1 inside the emulator: restart, which QEMU's -no-reboot
       turns into a clean exit. Anywhere else (real rabbit, user-mode QEMU)
       the probe just exits. */
    if (sc(NR_getpid, 0, 0, 0, 0, 0, 0) == 1)
        sc(NR_reboot, 0xfee1dead, 672274793, 0x01234567, 0, 0, 0);
    sc(NR_exit, 0, 0, 0, 0, 0, 0);
    for (;;) {}
}
