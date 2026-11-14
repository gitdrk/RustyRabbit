/* /init for the Linux 2.6.28 emulator test. Runs entirely inside the VM:
 *  - brings up loopback, serves /tone.* over a tiny HTTP server (child process)
 *  - acts as a minimal LMS on 127.0.0.1:3483
 *  - runs squeezelite -o - (PCM to a pipe), drops idle-silence runs (>= 32
 *    all-zero frames, plus leading zeros) and hashes the remaining PCM
 *  - prints one RESULT line per format, then powers the VM off.
 * The same hash computed on the PC (hash_raw.py) over the qemu-arm output and
 * the original WAV shows whether the old-kernel run decodes identically. */
#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <net/if.h>
#include <netinet/in.h>
#include <poll.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/reboot.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static double now(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec + t.tv_nsec / 1e9;
}

static void lo_up(void) {
    int s = socket(AF_INET, SOCK_DGRAM, 0);
    struct ifreq r;
    memset(&r, 0, sizeof r);
    strcpy(r.ifr_name, "lo");
    struct sockaddr_in *a = (struct sockaddr_in *)&r.ifr_addr;
    a->sin_family = AF_INET;
    a->sin_addr.s_addr = htonl(0x7f000001);
    if (ioctl(s, SIOCSIFADDR, &r)) perror("SIOCSIFADDR");
    ioctl(s, SIOCGIFFLAGS, &r);
    r.ifr_flags |= IFF_UP | IFF_RUNNING;
    if (ioctl(s, SIOCSIFFLAGS, &r)) perror("SIOCSIFFLAGS");
    close(s);
}

static int listen_on(int port) {
    int s = socket(AF_INET, SOCK_STREAM, 0), one = 1;
    setsockopt(s, SOL_SOCKET, SO_REUSEADDR, &one, sizeof one);
    struct sockaddr_in a = { .sin_family = AF_INET, .sin_port = htons(port), .sin_addr.s_addr = htonl(0x7f000001) };
    if (bind(s, (struct sockaddr *)&a, sizeof a) || listen(s, 4)) { perror("bind/listen"); exit(1); }
    return s;
}

static void http_server(int ls) {
    for (;;) {
        int c = accept(ls, 0, 0);
        if (c < 0) continue;
        char req[1024];
        int n = 0, k;
        while (n < (int)sizeof req - 1 && (k = read(c, req + n, sizeof req - 1 - n)) > 0) {
            n += k;
            req[n] = 0;
            if (strstr(req, "\r\n\r\n")) break;
        }
        char path[256] = "";
        sscanf(req, "GET %255s", path);
        int f = open(path, O_RDONLY);
        if (f < 0) { dprintf(c, "HTTP/1.0 404 Not Found\r\n\r\n"); close(c); continue; }
        off_t len = lseek(f, 0, SEEK_END);
        lseek(f, 0, SEEK_SET);
        dprintf(c, "HTTP/1.0 200 OK\r\nContent-Length: %ld\r\n\r\n", (long)len);
        char buf[16384];
        while ((k = read(f, buf, sizeof buf)) > 0) {
            char *p = buf;
            while (k > 0) { int w = write(c, p, k); if (w <= 0) break; p += w; k -= w; }
        }
        close(f);
        close(c);
    }
}

static void send_msg(int c, const char *op, const void *p, int n) {
    unsigned char h[6];
    h[0] = (n + 4) >> 8; h[1] = (n + 4) & 255;
    memcpy(h + 2, op, 4);
    write(c, h, 6);
    if (n) write(c, p, n);
}

static int read_full(int fd, void *b, int n) {
    int got = 0, k;
    while (got < n) { k = read(fd, (char *)b + got, n - got); if (k <= 0) return -1; got += k; }
    return 0;
}

static void run_one(char fmt, const char *path) {
    int ls = listen_on(3483), pfd[2];
    pipe(pfd);
    pid_t sl = fork();
    if (!sl) {
        dup2(pfd[1], 1); close(pfd[0]); close(pfd[1]); close(ls);
        execl("/squeezelite", "squeezelite", "-s", "127.0.0.1", "-o", "-", "-a", "16", "-m", "00:11:22:33:44:55",
              "-n", "vmtest", "-d", "all=warn", (char *)0);
        perror("exec squeezelite");
        _exit(127);
    }
    close(pfd[1]);
    double t0 = now();
    int c = accept(ls, 0, 0);
    close(ls);
    unsigned char hdr[8];
    static unsigned char body[4096];
    read_full(c, hdr, 8);
    unsigned n = ntohl(*(uint32_t *)(hdr + 4));
    read_full(c, body, n < sizeof body ? n : sizeof body);

    unsigned char aude[2] = { 1, 1 };
    send_msg(c, "aude", aude, 2);
    /* audg: old_gainL, old_gainR (unused), adjust=1, preamp, gainL=gainR=0x00010000 (unity, BE) */
    unsigned char audg[18] = { 0 };
    audg[8] = 1; audg[9] = 255; audg[11] = 1; audg[15] = 1;
    send_msg(c, "audg", audg, 18);
    unsigned char strm[24 + 256];
    memset(strm, 0, sizeof strm);
    strm[0] = 's'; strm[1] = '1'; strm[2] = fmt;
    strm[3] = strm[4] = strm[5] = strm[6] = '?';
    strm[7] = 32;            /* threshold KB */
    strm[10] = '0';          /* transition type */
    strm[18] = 8099 >> 8; strm[19] = 8099 & 255;  /* server_port (BE); server_ip 0 = same host */
    int hl = sprintf((char *)strm + 24, "GET %s HTTP/1.0\r\n\r\n", path);
    send_msg(c, "strm", strm, 24 + hl);

    fcntl(pfd[0], F_SETFL, O_NONBLOCK);
    fcntl(c, F_SETFL, O_NONBLOCK);
    uint32_t h = 2166136261u;
    long kept = 0, zrun = 0;
    int started = 0, got_d = 0, carry = 0;
    double last_audio = now();
    static unsigned char pcm[65536];
    unsigned char cb[4];
    char events[256] = "";
    unsigned char sbuf[512];
    int sfill = 0;
    while (now() - t0 < 600) {
        struct pollfd pf[2] = { { pfd[0], POLLIN, 0 }, { c, POLLIN, 0 } };
        poll(pf, 2, 200);
        int k = read(pfd[0], pcm, sizeof pcm);
        for (int i = 0; k > 0 && i < k; i++) {
            cb[carry++] = pcm[i];
            if (carry < 4) continue;
            carry = 0;
            if (!cb[0] && !cb[1] && !cb[2] && !cb[3]) { zrun++; continue; }
            if (started && zrun > 0 && zrun < 32) {
                for (long z = 0; z < zrun * 4; z++) h *= 16777619u;  /* hash of zero bytes */
                kept += zrun;
            }
            zrun = 0; started = 1;
            for (int b = 0; b < 4; b++) { h ^= cb[b]; h *= 16777619u; }
            kept++;
            last_audio = now();
        }
        int m = read(c, sbuf + sfill, sizeof sbuf - sfill);
        if (m > 0) sfill += m;
        while (sfill >= 8) {
            unsigned len = ntohl(*(uint32_t *)(sbuf + 4));
            if (sfill < 8 + (int)len) break;
            if (!memcmp(sbuf, "STAT", 4) && len >= 4) {
                char ev[5];
                memcpy(ev, sbuf + 8, 4); ev[4] = 0;
                if (strlen(events) < 200 && !strstr(events, ev)) { strcat(events, ev); strcat(events, " "); }
                if (!strcmp(ev, "STMd")) got_d = 1;
            }
            memmove(sbuf, sbuf + 8 + len, sfill - 8 - len);
            sfill -= 8 + len;
        }
        if (got_d && kept > 0 && now() - last_audio > 3) break;
    }
    kill(sl, SIGTERM);
    usleep(300000);
    kill(sl, SIGKILL);
    int st;
    waitpid(sl, &st, 0);
    close(c);
    close(pfd[0]);
    const char *how = "exited";
    if (WIFSIGNALED(st) && WTERMSIG(st) != SIGTERM && WTERMSIG(st) != SIGKILL) how = strsignal(WTERMSIG(st));
    else if (WIFSIGNALED(st)) how = "stopped-by-harness";
    printf("RESULT %s frames=%ld fnv=%08x events=[%s] %.1fs %s\n", path, kept, h, events, now() - t0, how);
}

int main(void) {
    setvbuf(stdout, 0, _IONBF, 0);
    printf("VMTEST BEGIN\n");
    lo_up();
    int hs = listen_on(8099);
    if (!fork()) http_server(hs);
    close(hs);
    run_one('f', "/tone.flac");
    run_one('m', "/tone.mp3");
    run_one('o', "/tone.ogg");
    printf("VMTEST END\n");
    sync();
    reboot(RB_POWER_OFF);
    return 0;
}
