//go:build darwin || linux || freebsd || openbsd || netbsd || dragonfly

package cli

import (
	"os"
	"syscall"
	"unsafe"
)

func isTerminal(value any) bool {
	file, ok := value.(*os.File)
	if !ok {
		return false
	}
	var state syscall.Termios
	_, _, err := syscall.Syscall(syscall.SYS_IOCTL, file.Fd(), terminalGetAttributes, uintptr(unsafe.Pointer(&state)))
	return err == 0
}
