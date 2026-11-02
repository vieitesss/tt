//go:build darwin || freebsd || openbsd || netbsd || dragonfly

package cli

import "syscall"

const terminalGetAttributes = syscall.TIOCGETA
