//go:build !darwin && !linux && !freebsd && !openbsd && !netbsd && !dragonfly

package cli

// Noninteractive behavior is safe when terminal detection is unavailable.
func isTerminal(any) bool { return false }
