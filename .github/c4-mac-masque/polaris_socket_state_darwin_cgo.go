//go:build darwin && cgo

package quic

/*
#include <errno.h>
#include <libproc.h>
#include <string.h>
#include <sys/proc_info.h>
#include <unistd.h>

_Static_assert(PROC_PIDFDSOCKETINFO == 3, "unexpected public socket-info flavor");
_Static_assert(SOI_S_CANTSENDMORE == 0x0010, "unexpected public socket state bit");
_Static_assert(PROC_PIDFDSOCKETINFO_SIZE == sizeof(struct socket_fdinfo),
               "public socket_fdinfo ABI mismatch");

// One self-PID, self-owned-FD query. No retry, callback, network or address export.
static int polaris_self_socket_state(int fd, int *cannot_send, int *pending_error,
                                     int *short_return) {
    struct socket_fdinfo info;
    memset(&info, 0, sizeof(info));
    *cannot_send = 0;
    *pending_error = 0;
    *short_return = 0;
    errno = 0;
    int n = proc_pidfdinfo(getpid(), fd, PROC_PIDFDSOCKETINFO,
                           &info, PROC_PIDFDSOCKETINFO_SIZE);
    if (n < 0) {
        return errno > 0 ? errno : EIO;
    }
    if (n != PROC_PIDFDSOCKETINFO_SIZE) {
        *short_return = 1;
        return 0;
    }
    *cannot_send = (info.psi.soi_state & SOI_S_CANTSENDMORE) != 0;
    *pending_error = (int)info.psi.soi_error;
    return 0;
}
*/
import "C"

import "syscall"

//go:noinline
func polarisReadSelfSocketState(fd int) (bool, int, bool, error) {
	var cannotSend, pendingError, shortReturn C.int
	code := C.polaris_self_socket_state(C.int(fd), &cannotSend, &pendingError,
		&shortReturn)
	if code != 0 {
		return false, 0, false, syscall.Errno(code)
	}
	if shortReturn != 0 {
		return false, 0, true, nil
	}
	return cannotSend != 0, int(pendingError), false, nil
}
