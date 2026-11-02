package cli

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestHelpAndVersionDoNotLaunchTheTUI(t *testing.T) {
	s := sandbox(t)
	for _, args := range [][]string{{}, {"--help"}} {
		code, stdout, stderr := s.run("", args...)
		if code != 0 || stderr != "" || !strings.Contains(stdout, "Usage:") || !strings.Contains(stdout, "apply") || strings.Contains(stdout, "--vault") || strings.Contains(stdout, "--projects") {
			t.Fatalf("help %v: %d %q %q", args, code, stdout, stderr)
		}
	}
	for _, flag := range []string{"--version", "-V"} {
		code, stdout, stderr := s.run("", flag)
		if code != 0 || stdout != "tt 0.4.2\n" || stderr != "" {
			t.Fatalf("version %s: %d %q %q", flag, code, stdout, stderr)
		}
	}
	for _, args := range [][]string{{"--projects"}, {"-p"}, {"list", "--projects"}, {"list", "-p"}} {
		code, stdout, stderr := s.run("", args...)
		if code != 2 || stdout != "" || !strings.Contains(stderr, "unknown shorthand flag") && !strings.Contains(stderr, "unknown flag") {
			t.Fatalf("unsupported projects flag %v: %d %q %q", args, code, stdout, stderr)
		}
	}
}

func TestUpdateUsesGoInstallWithoutTouchingRealEnvironment(t *testing.T) {
	s := sandbox(t)
	bin := filepath.Join(s.root, "bin")
	if err := os.Mkdir(bin, 0o755); err != nil {
		t.Fatal(err)
	}
	capture := filepath.Join(s.root, "update-args")
	script := "#!/bin/sh\nprintf '%s\\n' \"$GOBIN\" \"$*\" > \"$TT_UPDATE_CAPTURE\"\n"
	if err := os.WriteFile(filepath.Join(bin, "go"), []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", bin)
	t.Setenv("TT_UPDATE_CAPTURE", capture)
	for _, command := range []string{"update", "upgrade"} {
		code, stdout, stderr := s.run("", command)
		if code != 0 || stdout != "" || stderr != "" {
			t.Fatalf("%s: %d %q %q", command, code, stdout, stderr)
		}
		data, err := os.ReadFile(capture)
		want := filepath.Join(s.root, ".local", "bin") + "\ninstall github.com/vieitesss/tt/cmd/tt@main\n"
		if err != nil || string(data) != want {
			t.Fatalf("%s invoked %q: %v", command, data, err)
		}
	}
}
