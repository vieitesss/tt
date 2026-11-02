//go:build linux

package cli

import "syscall"

const terminalGetAttributes = syscall.TCGETS
