package project_test

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/vieitesss/tt/internal/project"
)

func TestConfigRoundTripPreservesProjectsPreferencesAndExtras(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	path := filepath.Join(home, "config", "config.toml")
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	original := "capture_target = 'abc'\nvault = '/legacy'\ncustom = 42\n[path_display]\nstyle = 'tail'\ntail = 2\n[[project]]\npath = '~/work'\nslug = 'work'\nnever_ask_nested = true\n"
	if err := os.WriteFile(path, []byte(original), 0o600); err != nil {
		t.Fatal(err)
	}
	config, err := project.LoadFrom(path)
	if err != nil {
		t.Fatal(err)
	}
	if config.CaptureTarget == nil || *config.CaptureTarget != "abc" || config.Display.Style != "tail" || config.Display.Tail != 2 || len(config.Projects) != 1 || config.Projects[0].Path != filepath.Join(home, "work") {
		t.Fatalf("unexpected config: %+v", config)
	}
	if err := config.Save(); err != nil {
		t.Fatal(err)
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	for _, part := range []string{"custom = 42", "vault = ", "~/work", "never_ask_nested = true", "tail = 2"} {
		if !strings.Contains(string(data), part) {
			t.Errorf("missing %q in %s", part, data)
		}
	}
	if _, err := project.LoadFrom(path); err != nil {
		t.Fatal(err)
	}
}
