/* Test-only intervention in iperf, never linked to EasyTier or installed globally.
 * Observes single-FD select waits on nonblocking sockets. Experimental mode
 * changes only those waits to a zero timeout; readiness is still kernel-derived.
 * Blocking control sockets retain their original timeout and behavior.
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/select.h>
#include <time.h>

static int (*original_select)(int, fd_set *, fd_set *, fd_set *, struct timeval *);
static unsigned long candidates, changed;
static int mode;

__attribute__((constructor)) static void init(void) {
    original_select = dlsym(RTLD_NEXT, "select");
    const char *value = getenv("ETCI_SELECT_INTERVENTION");
    mode = value && strcmp(value, "1") == 0;
    if (!original_select) _Exit(98);
}

int select(int nfds, fd_set *r, fd_set *w, fd_set *e, struct timeval *t) {
    int fd = -1, count = 0;
    if (r && !w && !e && t && (t->tv_sec > 0 || t->tv_usec > 0)) {
        for (int i=0; i<nfds && i<FD_SETSIZE; i++) {
            if (FD_ISSET(i,r)) {fd=i;count++;}
        }
    }
    if (count == 1 && (fcntl(fd,F_GETFL) & O_NONBLOCK)) {
        candidates++;
        if (candidates == 1) {
            Dl_info info = {0};
            dladdr(__builtin_return_address(0), &info);
            fprintf(stderr,"ETCI_SELECT first fd=%d timeout=%ld.%06ld caller=%s intervention=%d\n",
                    fd,(long)t->tv_sec,(long)t->tv_usec,info.dli_sname?info.dli_sname:"unknown",mode);
        }
        if (mode) {
            struct timeval now={0,0};
            changed++;
            return original_select(nfds,r,w,e,&now);
        }
    }
    return original_select(nfds,r,w,e,t);
}

__attribute__((destructor)) static void done(void) {
    fprintf(stderr,"ETCI_SELECT counts candidates=%lu changed=%lu intervention=%d\n",candidates,changed,mode);
}
