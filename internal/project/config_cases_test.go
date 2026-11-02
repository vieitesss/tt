package project_test

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/vieitesss/tt/internal/project"
)

func TestDataDirectoryUsesXDGThenHome(t *testing.T) {
	home := t.TempDir()
	xdg := filepath.Join(home, "data")
	t.Setenv("HOME", home)
	t.Setenv("XDG_DATA_HOME", xdg)
	if got := project.DataDir(); got != filepath.Join(xdg, "tt") {
		t.Fatalf("XDG data directory: %q", got)
	}
	t.Setenv("XDG_DATA_HOME", "")
	if got := project.DataDir(); got != filepath.Join(home, ".local", "share", "tt") {
		t.Fatalf("HOME data directory: %q", got)
	}
	t.Setenv("HOME", "")
	if got := project.DataDir(); got != "" {
		t.Fatalf("missing data directory: %q", got)
	}
}

func TestConfigDefaultsValidationAndSingleProject(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "missing", "config.toml")
	config, err := project.LoadFrom(path)
	if err != nil || config.Path != path || config.Display.Style != "short" || config.Display.Tail != 1 || len(config.Projects) != 0 {
		t.Fatalf("defaults: %+v %v", config, err)
	}
	if err := config.Save(); err != nil {
		t.Fatalf("save creates parents: %v", err)
	}
	withoutPath, err := project.LoadFrom("")
	if err != nil || withoutPath.Save() == nil {
		t.Fatal("saving without a path should fail")
	}
	for _, tc := range []struct{ name, content, field string }{
		{"capture target", `capture_target = "NOT VALID"`, "capture_target"},
		{"display style", "[path_display]\nstyle = 'fancy'\n", "path_display.style"},
		{"display tail", "[path_display]\nstyle = 'tail'\ntail = 0\n", "path_display.tail"},
		{"display key", "[path_display]\nunknown = 2\n", "path_display"},
		{"project missing slug", "[[project]]\npath = '/p/a'\n", "project.slug"},
		{"project bad type", "[[project]]\npath = 1\nslug = 'a'\n", "project.path"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if err := os.WriteFile(path, []byte(tc.content), 0o644); err != nil {
				t.Fatal(err)
			}
			if _, err := project.LoadFrom(path); err == nil || !strings.Contains(err.Error(), tc.field) {
				t.Fatalf("expected %s error: %v", tc.field, err)
			}
		})
	}
	if err := os.WriteFile(path, []byte("[project]\npath = '/p/one'\nslug = 'one'\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	config, err = project.LoadFrom(path)
	if err != nil || len(config.Projects) != 1 || config.Projects[0].Slug != "one" {
		t.Fatalf("single project: %+v %v", config, err)
	}
	if err := config.Save(); err != nil {
		t.Fatal(err)
	}
	if reloaded, err := project.LoadFrom(path); err != nil || len(reloaded.Projects) != 1 || reloaded.Projects[0] != config.Projects[0] {
		t.Fatalf("project round trip: %+v %v", reloaded, err)
	}
}

func TestConfigMigrationAndExplicitOverride(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("HOME", dir)
	t.Setenv("TT_CONFIG", "")
	t.Setenv("XDG_DATA_HOME", filepath.Join(dir, "data"))
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(dir, "legacy"))
	legacy := filepath.Join(dir, "legacy", "tt", "config.toml")
	shared := filepath.Join(dir, "data", "tt", "config.toml")
	if err := os.MkdirAll(filepath.Dir(legacy), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(legacy, []byte("capture_target = 'legacy'\n[[project]]\npath = '~/work'\nslug = 'stable'\nnever_ask_nested = true\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	override := filepath.Join(dir, "override.toml")
	t.Setenv("TT_CONFIG", override)
	config, err := project.Load()
	if err != nil || config.Path != override || config.CaptureTarget != nil {
		t.Fatalf("override: %+v %v", config, err)
	}
	if _, err := os.Stat(shared); !os.IsNotExist(err) {
		t.Fatalf("override triggered migration: %v", err)
	}
	t.Setenv("TT_CONFIG", "")
	config, err = project.Load()
	if err != nil || config.Path != shared || *config.CaptureTarget != "legacy" || len(config.Projects) != 1 || !config.Projects[0].NeverAskNested {
		t.Fatalf("migration: %+v %v", config, err)
	}
	if data, err := os.ReadFile(shared); err != nil || !strings.Contains(string(data), "~/work") {
		t.Fatalf("migrated config: %q %v", data, err)
	}
	if err := os.WriteFile(shared, []byte("capture_target = 'shared'\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	config, err = project.Load()
	if err != nil || config.CaptureTarget == nil || *config.CaptureTarget != "shared" {
		t.Fatalf("shared must win: %+v %v", config, err)
	}
}

func TestRegistrationRejectsMissingDirectoryBeforeWriting(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("XDG_DATA_HOME", filepath.Join(dir, "data"))
	configPath := filepath.Join(dir, "config.toml")
	config, err := project.LoadFrom(configPath)
	if err != nil {
		t.Fatal(err)
	}
	missing := filepath.Join(dir, "missing")
	if _, _, err := config.RegisterAndSave(missing); err == nil || !strings.Contains(err.Error(), missing) {
		t.Fatalf("missing directory: %v", err)
	}
	if len(config.Projects) != 0 {
		t.Fatalf("missing directory registered: %+v", config.Projects)
	}
	for _, path := range []string{configPath, filepath.Join(dir, "data", "tt")} {
		if _, err := os.Stat(path); !os.IsNotExist(err) {
			t.Errorf("unexpected write to %s: %v", path, err)
		}
	}
}

func TestSlugSelectionDoesNotHangOnFilesystemErrors(t *testing.T) {
	dir := t.TempDir()
	blocked := filepath.Join(dir, "file")
	if err := os.WriteFile(blocked, []byte("not a directory"), 0o644); err != nil {
		t.Fatal(err)
	}
	result := make(chan string, 1)
	go func() { result <- project.SlugFor(filepath.Join(dir, "work"), nil, blocked) }()
	select {
	case slug := <-result:
		if slug != "work" {
			t.Fatalf("slug: %s", slug)
		}
	case <-time.After(time.Second):
		t.Fatal("slug selection looped on a filesystem error")
	}
	t.Setenv("XDG_DATA_HOME", blocked)
	config, err := project.LoadFrom(filepath.Join(dir, "config.toml"))
	if err != nil {
		t.Fatal(err)
	}
	work := filepath.Join(dir, "work")
	if err := os.Mkdir(work, 0o755); err != nil {
		t.Fatal(err)
	}
	if _, _, err := config.Register(work); err == nil || !strings.Contains(err.Error(), "cannot create the store folder") {
		t.Fatalf("expected registration filesystem error: %v", err)
	}
}

func TestHomeSymlinkRoundTripAndRegistryCollisions(t *testing.T) {
	dir := t.TempDir()
	home := filepath.Join(dir, "real-home")
	alias := filepath.Join(dir, "home-alias")
	one := filepath.Join(home, "a", "app")
	two := filepath.Join(home, "b", "app")
	for _, p := range []string{one, two} {
		if err := os.MkdirAll(p, 0o755); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.Symlink(home, alias); err != nil {
		t.Fatal(err)
	}
	t.Setenv("HOME", alias)
	t.Setenv("XDG_DATA_HOME", filepath.Join(dir, "data"))
	config, err := project.LoadFrom(filepath.Join(dir, "config.toml"))
	if err != nil {
		t.Fatal(err)
	}
	if err := config.SetNeverAsk(one); err != nil {
		t.Fatalf("unregistered preference should be ignored: %v", err)
	}
	if _, err := os.Stat(config.Path); !os.IsNotExist(err) {
		t.Fatalf("unregistered preference wrote config: %v", err)
	}
	if err := os.MkdirAll(filepath.Join(dir, "data", "tt", "app"), 0o755); err != nil {
		t.Fatal(err)
	}
	first, created, err := config.RegisterAndSave(one)
	if err != nil || !created || first.Slug != "app-2" {
		t.Fatalf("orphan store collision: %+v %t %v", first, created, err)
	}
	again, created, err := config.RegisterAndSave(filepath.Join(alias, "a", "app"))
	if err != nil || created || again.Slug != first.Slug {
		t.Fatalf("symlink idempotence: %+v %t %v", again, created, err)
	}
	if err := config.SetNeverAsk(filepath.Join(alias, "a", "app")); err != nil {
		t.Fatal(err)
	}
	if reloaded, err := project.LoadFrom(config.Path); err != nil || len(reloaded.Projects) != 1 || !reloaded.Projects[0].NeverAskNested {
		t.Fatalf("symlink preference not persisted: %+v %v", reloaded, err)
	}
	second, created, err := config.RegisterAndSave(two)
	if err != nil || !created || second.Slug != "app-3" {
		t.Fatalf("slug collision: %+v %t %v", second, created, err)
	}
	data, err := os.ReadFile(config.Path)
	if err != nil || !strings.Contains(string(data), "~/a/app") {
		t.Fatalf("home serialization: %q %v", data, err)
	}
	if project.Nonportable(one) || !project.Nonportable(dir) {
		t.Fatal("portable paths do not respect symlinked HOME")
	}
	if removed, ok := config.Remove(filepath.Join(alias, "a", "app")); !ok || removed.Slug != first.Slug {
		t.Fatalf("remove by alias: %+v %t", removed, ok)
	}
	if removed, ok := config.Remove(second.Slug); !ok || removed.Slug != second.Slug {
		t.Fatalf("remove by slug: %+v %t", removed, ok)
	}
}
