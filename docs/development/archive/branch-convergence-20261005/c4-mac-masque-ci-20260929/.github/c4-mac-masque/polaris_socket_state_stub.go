//go:build !darwin || !cgo

package quic

import "syscall"

// The reviewed candidate is Darwin/arm64/CGO1. Other build faces cannot query.
//
//go:noinline
func polarisReadSelfSocketState(fd int) (bool, int, bool, error) {
	return false, 0, false, syscall.ENOTSUP
}
