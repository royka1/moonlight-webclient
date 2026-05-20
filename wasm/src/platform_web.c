// Full replacement for moonlight-common-c/src/PlatformSockets.c on the
// emscripten target. We provide every symbol the header declares, so the
// linker has no use for the upstream .c file (it's excluded from the
// build in CMakeLists.txt).
//
// All socket I/O is multiplexed through a single WebTransport/WebSocket
// session to the host-side proxy. The "SOCKET" handles we hand out are
// small integers (1..MLW_MAX_CHANNELS) identifying channels in the table.
//
// Threading: moonlight-common-c spawns pthreads internally. Those run in
// emscripten pthread workers. mlw_inbound_packet() is invoked by JS from
// the main wasm worker. The channel table uses pthread_mutex + pthread_cond
// (which emscripten implements on top of SharedArrayBuffer + Atomics.wait
// + futex) so producer and consumer can be on different workers.

#include "bindings.h"

#include <errno.h>
#include <pthread.h>
#include <stdarg.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <netinet/in.h>
#include <arpa/inet.h>
#include <poll.h>
#include <fcntl.h>

// Forward declaration — poll() override below calls it.
int pollSockets(struct pollfd* pollFds, int pollFdsCount, int timeoutMs);

// Transport callbacks. Pthread workers route to the main wasm worker via
// postMessage; see moonlight_imports.js for the routing logic and
// src/wasm/worker.ts for the receiving side.
#define MLW_TRANSPORT_OPEN(ch, host, port, proto) \
    mlw_js_transport_open((ch), (host), (port), (proto))
#define MLW_TRANSPORT_CLOSE(ch) \
    mlw_js_transport_close((ch))
#define MLW_TRANSPORT_SEND(ch, data, len) \
    mlw_js_transport_send((ch), (const uint8_t*)(data), (int)(len))

// Match the types moonlight-common-c uses without dragging its headers in.
typedef int SOCKET;
typedef ssize_t SOCK_RET;
typedef int SOCKADDR_LEN;
#define LAST_SOCKET_ERROR() errno

#define MLW_MAX_CHANNELS 32
#define MLW_PROTO_NONE 0
#define MLW_PROTO_UDP  1
#define MLW_PROTO_TCP  2

typedef struct mlw_pkt {
    uint8_t* data;
    int      len;
    int      offset;
    struct mlw_pkt* next;
} mlw_pkt_t;

typedef struct {
    atomic_int      in_use;
    int             proto;
    int             port;
    char            host[256];
    int             opened;
    int             eof;          // remote host closed the connection
    mlw_pkt_t*      rx_head;
    mlw_pkt_t*      rx_tail;
    pthread_mutex_t lock;
    pthread_cond_t  cond;
} mlw_channel_t;

static mlw_channel_t s_channels[MLW_MAX_CHANNELS];
static pthread_once_t s_init_once = PTHREAD_ONCE_INIT;
static char s_pending_hostname[256];  // single-shot from resolveHostName

static void mlw_init_once(void) {
    for (int i = 0; i < MLW_MAX_CHANNELS; i++) {
        pthread_mutex_init(&s_channels[i].lock, NULL);
        pthread_cond_init(&s_channels[i].cond, NULL);
    }
}

static int channel_is_ours(SOCKET fd) {
    return fd >= 1 && fd < MLW_MAX_CHANNELS && atomic_load(&s_channels[fd].in_use);
}

static int channel_alloc(int proto) {
    pthread_once(&s_init_once, mlw_init_once);
    for (int i = 1; i < MLW_MAX_CHANNELS; i++) {
        int expected = 0;
        if (atomic_compare_exchange_strong(&s_channels[i].in_use, &expected, 1)) {
            s_channels[i].proto = proto;
            s_channels[i].port = 0;
            s_channels[i].host[0] = 0;
            s_channels[i].opened = 0;
            s_channels[i].eof = 0;
            pthread_mutex_lock(&s_channels[i].lock);
            while (s_channels[i].rx_head) {
                mlw_pkt_t* p = s_channels[i].rx_head;
                s_channels[i].rx_head = p->next;
                free(p->data);
                free(p);
            }
            s_channels[i].rx_tail = NULL;
            pthread_mutex_unlock(&s_channels[i].lock);
            return i;
        }
    }
    errno = EMFILE;
    return -1;
}

static void channel_open(SOCKET fd, const char* host, int port) {
    if (!channel_is_ours(fd) || s_channels[fd].opened) return;
    strncpy(s_channels[fd].host, host, sizeof(s_channels[fd].host) - 1);
    s_channels[fd].port = port;
    s_channels[fd].opened = 1;
    fprintf(stderr, "[plat] open ch=%d %s:%d proto=%d\n",
            fd, s_channels[fd].host, port, s_channels[fd].proto);
    MLW_TRANSPORT_OPEN(fd, s_channels[fd].host, port, s_channels[fd].proto);
}

static void channel_close_reason(SOCKET fd, const char* reason) {
    if (!channel_is_ours(fd)) return;
    fprintf(stderr, "[plat] CLOSE ch=%d proto=%d reason=%s eof=%d\n",
            fd, s_channels[fd].proto, reason, s_channels[fd].eof);
    if (s_channels[fd].opened) {
        MLW_TRANSPORT_CLOSE(fd);
        s_channels[fd].opened = 0;
    }
    pthread_mutex_lock(&s_channels[fd].lock);
    while (s_channels[fd].rx_head) {
        mlw_pkt_t* p = s_channels[fd].rx_head;
        s_channels[fd].rx_head = p->next;
        free(p->data);
        free(p);
    }
    s_channels[fd].rx_tail = NULL;
    pthread_cond_broadcast(&s_channels[fd].cond);
    pthread_mutex_unlock(&s_channels[fd].lock);
    atomic_store(&s_channels[fd].in_use, 0);
}

static void channel_close(SOCKET fd) {
    channel_close_reason(fd, "closeSocket");
}

void mlw_inbound_packet(int channel, const uint8_t* data, int len) {
    if (!channel_is_ours(channel)) {
        fprintf(stderr, "[plat] inbound on bad ch=%d (len=%d)\n", channel, len);
        return;
    }
    // Zero-length packet signals EOF (remote host closed the connection).
    if (len == 0) {
        fprintf(stderr, "[plat] inbound EOF ch=%d\n", channel);
        pthread_mutex_lock(&s_channels[channel].lock);
        s_channels[channel].eof = 1;
        pthread_cond_signal(&s_channels[channel].cond);
        pthread_mutex_unlock(&s_channels[channel].lock);
        return;
    }
    mlw_pkt_t* p = (mlw_pkt_t*)malloc(sizeof(*p));
    if (!p) return;
    p->data = (uint8_t*)malloc(len);
    if (!p->data) { free(p); return; }
    memcpy(p->data, data, len);
    p->len = len;
    p->offset = 0;
    p->next = NULL;
    pthread_mutex_lock(&s_channels[channel].lock);
    if (s_channels[channel].rx_tail) {
        s_channels[channel].rx_tail->next = p;
    } else {
        s_channels[channel].rx_head = p;
    }
    s_channels[channel].rx_tail = p;
    pthread_cond_signal(&s_channels[channel].cond);
    pthread_mutex_unlock(&s_channels[channel].lock);
}

static int channel_recv(SOCKET fd, void* buf, int size, int is_datagram, int timeout_ms) {
    if (!channel_is_ours(fd)) { errno = EBADF; return -1; }
    pthread_mutex_lock(&s_channels[fd].lock);

    if (timeout_ms > 0 && !s_channels[fd].rx_head && !s_channels[fd].eof) {
        struct timespec ts;
        clock_gettime(CLOCK_REALTIME, &ts);
        ts.tv_sec += timeout_ms / 1000;
        ts.tv_nsec += (timeout_ms % 1000) * 1000000L;
        if (ts.tv_nsec >= 1000000000L) { ts.tv_sec++; ts.tv_nsec -= 1000000000L; }
        while (atomic_load(&s_channels[fd].in_use) && !s_channels[fd].rx_head && !s_channels[fd].eof) {
            if (pthread_cond_timedwait(&s_channels[fd].cond, &s_channels[fd].lock, &ts) != 0)
                break;
        }
    } else if (timeout_ms != 0) {
        while (atomic_load(&s_channels[fd].in_use) && !s_channels[fd].rx_head && !s_channels[fd].eof) {
            pthread_cond_wait(&s_channels[fd].cond, &s_channels[fd].lock);
        }
    }

    if (!atomic_load(&s_channels[fd].in_use)) {
        pthread_mutex_unlock(&s_channels[fd].lock);
        errno = EBADF;
        return -1;
    }
    if (!s_channels[fd].rx_head) {
        // No data queued. If the remote host closed the connection, signal EOF.
        int is_eof = s_channels[fd].eof;
        pthread_mutex_unlock(&s_channels[fd].lock);
        if (is_eof) return 0;
        errno = EAGAIN;
        return -1;
    }
    mlw_pkt_t* p = s_channels[fd].rx_head;
    int available = p->len - p->offset;
    int n = available < size ? available : size;
    memcpy(buf, p->data + p->offset, n);
    if (is_datagram || n == available) {
        s_channels[fd].rx_head = p->next;
        if (!s_channels[fd].rx_head) s_channels[fd].rx_tail = NULL;
        free(p->data);
        free(p);
    } else {
        p->offset += n;
    }
    pthread_mutex_unlock(&s_channels[fd].lock);
    return n;
}

// Override socket() — ENet calls socket(AF_INET, SOCK_DGRAM, 0) via
// enet_socket_create(). Emscripten's libc would create a SOCKFS node whose
// fd our channel-based sendto/recvfrom/poll overrides don't recognise.
// Route SOCK_DGRAM → UDP channel, SOCK_STREAM → TCP channel.
int socket(int domain, int type, int protocol) {
    (void)domain; (void)protocol;
    int proto;
    if (type == SOCK_DGRAM) {
        proto = MLW_PROTO_UDP;
    } else if (type == SOCK_STREAM) {
        proto = MLW_PROTO_TCP;
    } else {
        errno = EPROTONOSUPPORT;
        return -1;
    }
    int fd = channel_alloc(proto);
    if (fd < 0) { errno = EMFILE; return -1; }
    return fd;
}

// bind() — ENet calls bind() on the socket created via socket() above.
// Our virtual channels don't need binding.  Return 0 (success) for our
// channels so the caller proceeds normally.
int bind(int sockfd, const struct sockaddr* addr, socklen_t addrlen) {
    (void)addr; (void)addrlen;
    if (channel_is_ours(sockfd)) return 0;
    errno = ENOTSOCK;
    return -1;
}

// poll() — enet_socket_wait() calls poll() to wait for socket readiness.
// Forward to our pollSockets() which knows how to wait on channel queues.
int poll(struct pollfd* fds, nfds_t nfds, int timeout) {
    return pollSockets(fds, (int)nfds, timeout);
}

// getsockname() — enet_socket_get_address() calls getsockname() after
// bind() to retrieve the local address.  Return a zero address (INADDR_ANY
// + port 0) to signal "wildcard bound".
int getsockname(int sockfd, struct sockaddr* addr, socklen_t* addrlen) {
    if (!channel_is_ours(sockfd)) { errno = ENOTSOCK; return -1; }
    if (*addrlen >= (socklen_t)sizeof(struct sockaddr_in)) {
        struct sockaddr_in* a = (struct sockaddr_in*)addr;
        memset(a, 0, sizeof(*a));
        a->sin_family = AF_INET;
        a->sin_addr.s_addr = INADDR_ANY;
        a->sin_port = 0;
        *addrlen = sizeof(*a);
    }
    return 0;
}

// fcntl() — enet_socket_set_option(ENET_SOCKOPT_NONBLOCK) uses fcntl()
// to set O_NONBLOCK.  Our channels are always non-blocking by design.
// Accept F_GETFL and F_SETFL; reject everything else.
int fcntl(int fd, int cmd, ...) {
    if (channel_is_ours(fd)) {
        if (cmd == F_GETFL) return O_NONBLOCK;
        if (cmd == F_SETFL) return 0;
        errno = EINVAL;
        return -1;
    }
    errno = EBADF;
    return -1;
}

// setsockopt() — ENet sets RCVBUF, SNDBUF, REUSEADDR, QOS, etc.
// These don't apply to our virtual channels; silently accept them.
int setsockopt(int sockfd, int level, int optname,
               const void* optval, socklen_t optlen) {
    (void)level; (void)optname; (void)optval; (void)optlen;
    if (channel_is_ours(sockfd)) return 0;
    errno = ENOTSOCK;
    return -1;
}

// Override emscripten's libc recv() — our channel FDs are just small
// integers (1..MLW_MAX_CHANNELS) that the libc socket layer doesn't know
// about. Channel 0 is stdout, so this override also catches that as EBADF.
SOCK_RET recv(SOCKET sock, void* buf, size_t len, int flags) {
    (void)flags;
    errno = 0;
    if (!channel_is_ours(sock)) { errno = ENOTSOCK; return -1; }
    // TCP is stream; UDP has its own recvUdpSocket entry point.
    return channel_recv(sock, buf, (int)len, /*is_datagram=*/0, -1);
}

// send() is used by moonlight-common-c for non-mtu-safe writes on TCP
// (mainly ENET keep-alive). Route it to our transport.
SOCK_RET send(SOCKET sock, const void* buf, size_t len, int flags) {
    (void)flags;
    errno = 0;
    if (!channel_is_ours(sock)) { errno = EBADF; return -1; }
    int r = MLW_TRANSPORT_SEND(sock, (const uint8_t*)buf, (int)len);
    if (r < 0) { errno = EAGAIN; return -1; }
    return r;
}

// Override sendto / recvfrom — ENet uses these for UDP sockets created
// by bindUdpSocket() or socket(). Without these, emscripten's libc routes
// them to WebSocket emulation (createPeer) which is blocked (mixed content).
//
// On the first sendto() for an unopened channel we extract the target
// host:port from dest_addr and send an OPEN frame to the proxy.  ENet
// doesn't call connect() before sending, so this is the earliest point we
// know the destination.
SOCK_RET sendto(SOCKET sock, const void* buf, size_t len, int flags,
                const struct sockaddr* dest_addr, socklen_t addrlen) {
    (void)flags; (void)addrlen;
    errno = 0;
    if (!channel_is_ours(sock)) { errno = ENOTSOCK; return -1; }

    // Lazy-open: extract host:port from the destination address.
    if (!s_channels[sock].opened && dest_addr != NULL) {
        if (dest_addr->sa_family == AF_INET) {
            const struct sockaddr_in* a = (const struct sockaddr_in*)dest_addr;
            inet_ntop(AF_INET, &a->sin_addr,
                      s_channels[sock].host, sizeof(s_channels[sock].host));
            channel_open(sock, s_channels[sock].host, ntohs(a->sin_port));
        } else if (dest_addr->sa_family == AF_INET6) {
            const struct sockaddr_in6* a = (const struct sockaddr_in6*)dest_addr;
            inet_ntop(AF_INET6, &a->sin6_addr,
                      s_channels[sock].host, sizeof(s_channels[sock].host));
            channel_open(sock, s_channels[sock].host, ntohs(a->sin6_port));
        }
    }

    int r = MLW_TRANSPORT_SEND(sock, (const uint8_t*)buf, (int)len);
    if (r < 0) { errno = EAGAIN; return -1; }
    return r;
}

SOCK_RET recvfrom(SOCKET sock, void* buf, size_t len, int flags,
                  struct sockaddr* src_addr, socklen_t* addrlen) {
    (void)flags; (void)src_addr; (void)addrlen;
    errno = 0;
    if (!channel_is_ours(sock)) { errno = ENOTSOCK; return -1; }
    // Non-blocking: ENet does its own polling via enet_socket_wait().
    return channel_recv(sock, buf, (int)len, /*is_datagram=*/1, /*timeout_ms=*/0);
}

// ENet calls connect() on UDP sockets after bindUdpSocket(). Extract the
// target host:port and open the channel so the proxy knows about it.
int connect(SOCKET sock, const struct sockaddr* addr, socklen_t addrlen) {
    (void)addrlen;
    if (!channel_is_ours(sock)) { errno = ENOTSOCK; return -1; }

    // Skip if already opened (e.g. TCP channels opened by connectTcpSocket).
    if (s_channels[sock].opened) return 0;

    if (addr->sa_family == AF_INET) {
        struct sockaddr_in* a = (struct sockaddr_in*)addr;
        inet_ntop(AF_INET, &a->sin_addr, s_channels[sock].host, sizeof(s_channels[sock].host));
        channel_open(sock, s_channels[sock].host, ntohs(a->sin_port));
    } else if (addr->sa_family == AF_INET6) {
        struct sockaddr_in6* a = (struct sockaddr_in6*)addr;
        inet_ntop(AF_INET6, &a->sin6_addr, s_channels[sock].host, sizeof(s_channels[sock].host));
        channel_open(sock, s_channels[sock].host, ntohs(a->sin6_port));
    }
    return 0;
}

// Emscripten's sendmsg/recvmsg may be called from enet when HAS_MSGHDR_FLAGS
// is defined. We force NO_MSGAPI=1 so the sendto/recvfrom path is used
// instead; these stubs exist as a safety net in case some other code path
// calls them directly.
SOCK_RET sendmsg(SOCKET sock, const struct msghdr* msg, int flags) {
    (void)flags;
    errno = 0;
    if (!channel_is_ours(sock)) { errno = ENOTSOCK; return -1; }

    // Lazy-open: extract host:port from msg_name (same semantics as sendto).
    if (!s_channels[sock].opened && msg != NULL && msg->msg_name != NULL) {
        const struct sockaddr* addr = (const struct sockaddr*)msg->msg_name;
        if (addr->sa_family == AF_INET) {
            const struct sockaddr_in* a = (const struct sockaddr_in*)addr;
            inet_ntop(AF_INET, &a->sin_addr,
                      s_channels[sock].host, sizeof(s_channels[sock].host));
            channel_open(sock, s_channels[sock].host, ntohs(a->sin_port));
        } else if (addr->sa_family == AF_INET6) {
            const struct sockaddr_in6* a = (const struct sockaddr_in6*)addr;
            inet_ntop(AF_INET6, &a->sin6_addr,
                      s_channels[sock].host, sizeof(s_channels[sock].host));
            channel_open(sock, s_channels[sock].host, ntohs(a->sin6_port));
        }
    }

    // Gather all iov fragments and send as one chunk.
    int total = 0;
    for (int i = 0; i < (int)msg->msg_iovlen; i++) {
        total += msg->msg_iov[i].iov_len;
    }
    if (total == 0) return 0;
    uint8_t* flat = (uint8_t*)malloc(total);
    if (!flat) { errno = ENOMEM; return -1; }
    int off = 0;
    for (int i = 0; i < (int)msg->msg_iovlen; i++) {
        memcpy(flat + off, msg->msg_iov[i].iov_base, msg->msg_iov[i].iov_len);
        off += msg->msg_iov[i].iov_len;
    }
    int r = MLW_TRANSPORT_SEND(sock, flat, total);
    free(flat);
    if (r < 0) { errno = EAGAIN; return -1; }
    return r;
}

SOCK_RET recvmsg(SOCKET sock, struct msghdr* msg, int flags) {
    (void)msg; (void)flags;
    if (!channel_is_ours(sock)) { errno = ENOTSOCK; return -1; }
    // Minimal stub: ENet with HAS_MSGHDR_FLAGS=0 won't call this.
    errno = ENOSYS;
    return -1;
}

// =====================================================================
//   PlatformSockets.h surface
// =====================================================================

void addrToUrlSafeString(struct sockaddr_storage* addr, char* string, size_t stringLength) {
    char tmp[INET6_ADDRSTRLEN];
    if (addr->ss_family == AF_INET) {
        struct sockaddr_in* a = (struct sockaddr_in*)addr;
        inet_ntop(AF_INET, &a->sin_addr, tmp, sizeof(tmp));
        snprintf(string, stringLength, "%s", tmp);
    } else if (addr->ss_family == AF_INET6) {
        struct sockaddr_in6* a = (struct sockaddr_in6*)addr;
        inet_ntop(AF_INET6, &a->sin6_addr, tmp, sizeof(tmp));
        snprintf(string, stringLength, "[%s]", tmp);
    } else {
        snprintf(string, stringLength, "?");
    }
}

void shutdownTcpSocket(SOCKET s) {
    channel_close_reason(s, "shutdownTcpSocket");
}

int setNonFatalRecvTimeoutMs(SOCKET s, int timeoutMs) {
    (void)s; (void)timeoutMs;
    return 0;
}

int pollSockets(struct pollfd* pollFds, int pollFdsCount, int timeoutMs) {
    // moonlight-common-c uses this for select-style multiplexing. We
    // approximate by polling each channel's queue with a short timeout
    // distributed across the channels. Good enough for the few places
    // moonlight uses it.
    errno = 0;
    int ready = 0;
    int per = pollFdsCount > 0 ? (timeoutMs >= 0 ? timeoutMs / pollFdsCount : -1) : 0;
    for (int i = 0; i < pollFdsCount; i++) {
        pollFds[i].revents = 0;
        SOCKET s = pollFds[i].fd;
        if (!channel_is_ours(s)) continue;
        pthread_mutex_lock(&s_channels[s].lock);
        int readable = s_channels[s].rx_head != NULL || s_channels[s].eof;
        pthread_mutex_unlock(&s_channels[s].lock);
        if (readable && (pollFds[i].events & POLLIN)) {
            pollFds[i].revents |= POLLIN;
            ready++;
        } else if (per != 0 && (pollFds[i].events & POLLIN)) {
            // Block for `per` ms waiting for data or EOF.
            pthread_mutex_lock(&s_channels[s].lock);
            if (per > 0) {
                struct timespec ts;
                clock_gettime(CLOCK_REALTIME, &ts);
                ts.tv_sec += per / 1000;
                ts.tv_nsec += (per % 1000) * 1000000L;
                if (ts.tv_nsec >= 1000000000L) { ts.tv_sec++; ts.tv_nsec -= 1000000000L; }
                while (atomic_load(&s_channels[s].in_use) && !s_channels[s].rx_head && !s_channels[s].eof) {
                    if (pthread_cond_timedwait(&s_channels[s].cond, &s_channels[s].lock, &ts) != 0)
                        break;
                }
            } else {
                while (atomic_load(&s_channels[s].in_use) && !s_channels[s].rx_head && !s_channels[s].eof)
                    pthread_cond_wait(&s_channels[s].cond, &s_channels[s].lock);
            }
            if (s_channels[s].rx_head || s_channels[s].eof) {
                pollFds[i].revents |= POLLIN;
                ready++;
            }
            pthread_mutex_unlock(&s_channels[s].lock);
        }
    }
    return ready;
}

bool isSocketReadable(SOCKET s) {
    if (!channel_is_ours(s)) return false;
    pthread_mutex_lock(&s_channels[s].lock);
    int readable = s_channels[s].rx_head != NULL || s_channels[s].eof;
    pthread_mutex_unlock(&s_channels[s].lock);
    return readable;
}

int recvUdpSocket(SOCKET s, char* buffer, int size, bool useSelect) {
    (void)useSelect;
    errno = 0;
    return channel_recv(s, buffer, size, /*is_datagram=*/1, -1);
}

void closeSocket(SOCKET s) {
    channel_close(s);
}

SOCKET bindUdpSocket(int addressFamily, struct sockaddr_storage* localAddr,
                    SOCKADDR_LEN addrLen, int bufferSize, int socketQosType) {
    (void)addressFamily; (void)localAddr; (void)addrLen;
    (void)bufferSize; (void)socketQosType;
    return channel_alloc(MLW_PROTO_UDP);
}

int setSocketNonBlocking(SOCKET s, bool enabled) {
    (void)s; (void)enabled;
    return 0;
}

SOCKET createSocket(int addressFamily, int socketType, int protocol, bool nonBlocking) {
    (void)addressFamily; (void)protocol; (void)nonBlocking;
    int proto = (socketType == SOCK_STREAM) ? MLW_PROTO_TCP : MLW_PROTO_UDP;
    return channel_alloc(proto);
}

SOCKET connectTcpSocket(struct sockaddr_storage* dstaddr, SOCKADDR_LEN addrlen,
                       unsigned short port, int timeoutSec) {
    (void)addrlen; (void)timeoutSec;
    char host[INET6_ADDRSTRLEN] = {0};
    if (dstaddr->ss_family == AF_INET) {
        struct sockaddr_in* a = (struct sockaddr_in*)dstaddr;
        if (a->sin_addr.s_addr == 0 && s_pending_hostname[0]) {
            strncpy(host, s_pending_hostname, sizeof(host) - 1);
        } else {
            inet_ntop(AF_INET, &a->sin_addr, host, sizeof(host));
        }
    } else if (dstaddr->ss_family == AF_INET6) {
        struct sockaddr_in6* a = (struct sockaddr_in6*)dstaddr;
        inet_ntop(AF_INET6, &a->sin6_addr, host, sizeof(host));
    } else {
        errno = EAFNOSUPPORT;
        return -1;
    }
    SOCKET fd = channel_alloc(MLW_PROTO_TCP);
    if (fd < 0) return -1;
    channel_open(fd, host, port);
    return fd;
}

int getLocalAddressByUdpConnect(const struct sockaddr_storage* targetAddr,
                               SOCKADDR_LEN targetAddrLen,
                               unsigned short targetPort,
                               struct sockaddr_storage* localAddr,
                               SOCKADDR_LEN* localAddrLen) {
    // We never have a meaningful local address - the proxy hides that
    // detail. Return 0.0.0.0:0 so moonlight-common-c's NAT-traversal
    // helpers see something well-formed but harmless.
    (void)targetAddr; (void)targetAddrLen; (void)targetPort;
    memset(localAddr, 0, sizeof(*localAddr));
    struct sockaddr_in* a = (struct sockaddr_in*)localAddr;
    a->sin_family = AF_INET;
    a->sin_port = 0;
    a->sin_addr.s_addr = 0;
    if (localAddrLen) *localAddrLen = sizeof(*a);
    return 0;
}

int sendMtuSafe(SOCKET s, char* buffer, int size) {
    errno = 0;
    if (!channel_is_ours(s)) { errno = EBADF; return -1; }
    int r = MLW_TRANSPORT_SEND(s, buffer, size);
    if (r < 0) { errno = EAGAIN; return -1; }
    return r;
}

int enableNoDelay(SOCKET s) {
    (void)s;
    return 0;
}

int resolveHostName(const char* host, int family, int tcpTestPort,
                   struct sockaddr_storage* addr, SOCKADDR_LEN* addrLen) {
    (void)family; (void)tcpTestPort;
    memset(addr, 0, sizeof(*addr));
    struct sockaddr_in* a = (struct sockaddr_in*)addr;
    a->sin_family = AF_INET;
    if (inet_pton(AF_INET, host, &a->sin_addr) == 1) {
        if (addrLen) *addrLen = sizeof(*a);
        return 0;
    }
    // Hostname - the proxy will DNS-resolve it. Stash it and let
    // connectTcpSocket() recover it via the sin_addr=0 sentinel.
    strncpy(s_pending_hostname, host, sizeof(s_pending_hostname) - 1);
    s_pending_hostname[sizeof(s_pending_hostname) - 1] = 0;
    a->sin_addr.s_addr = 0;
    if (addrLen) *addrLen = sizeof(*a);
    return 0;
}

bool isNat64SynthesizedAddress(struct sockaddr_storage* address) {
    (void)address;
    return false;
}

bool isPrivateNetworkAddress(struct sockaddr_storage* address) {
    if (address->ss_family == AF_INET) {
        struct sockaddr_in* a = (struct sockaddr_in*)address;
        uint32_t ip = ntohl(a->sin_addr.s_addr);
        return ((ip & 0xff000000) == 0x0a000000)      // 10.0.0.0/8
            || ((ip & 0xfff00000) == 0xac100000)      // 172.16.0.0/12
            || ((ip & 0xffff0000) == 0xc0a80000)      // 192.168.0.0/16
            || ((ip & 0xff000000) == 0x7f000000);     // 127.0.0.0/8
    }
    return false;
}

void enterLowLatencyMode(void) {}
void exitLowLatencyMode(void) {}

int initializePlatformSockets(void) {
    pthread_once(&s_init_once, mlw_init_once);
    fprintf(stderr, "[plat] initializePlatformSockets ok\n");
    return 0;
}

void cleanupPlatformSockets(void) {
    for (int i = 1; i < MLW_MAX_CHANNELS; i++) {
        if (atomic_load(&s_channels[i].in_use)) {
            channel_close_reason(i, "cleanupPlatformSockets");
        }
    }
    fprintf(stderr, "[plat] cleanupPlatformSockets ok\n");
}
