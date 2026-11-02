package project

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/vieitesss/tt/internal/fsutil"
)

func TestMigrationCollisionLoadsTheRacingWriter(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("HOME", dir)
	t.Setenv("TT_CONFIG", "")
	t.Setenv("XDG_DATA_HOME", filepath.Join(dir, "data"))
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(dir, "legacy"))
	legacy := filepath.Join(dir, "legacy", "tt", "config.toml")
	if err := os.MkdirAll(filepath.Dir(legacy), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(legacy, []byte("capture_target = 'legacy'\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	winner := []byte("capture_target = 'winner'\n")
	config, err := load(func(path string, data []byte, mode fsutil.Mode) (bool, error) {
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			return false, err
		}
		if err := os.WriteFile(path, winner, 0o644); err != nil {
			return false, err
		}
		return fsutil.Write(path, data, mode)
	})
	if err != nil || config.CaptureTarget == nil || *config.CaptureTarget != "winner" {
		t.Fatalf("migration did not load racing winner: %+v %v", config, err)
	}
	shared := filepath.Join(dir, "data", "tt", "config.toml")
	if data, err := os.ReadFile(shared); err != nil || string(data) != string(winner) {
		t.Fatalf("migration overwrote racing winner: %q %v", data, err)
	}
}
