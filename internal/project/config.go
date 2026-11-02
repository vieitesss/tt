// Package project manages configuration, the project registry, and resolution.
package project

import (
	"bytes"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"strings"

	"github.com/BurntSushi/toml"
	"github.com/vieitesss/tt/internal/fsutil"
	"github.com/vieitesss/tt/internal/vault"
)

type Project struct {
	Path           string `json:"path"`
	Slug           string `json:"slug"`
	NeverAskNested bool   `json:"never_ask_nested"`
}

type PathDisplay struct {
	Style string
	Tail  int64
}

type Config struct {
	Path            string
	CaptureTarget   *string
	DeprecatedVault *string
	Display         PathDisplay
	Projects        []Project
	extra           map[string]any
}

func defaultConfig(path string) *Config {
	return &Config{Path: path, Display: PathDisplay{Style: "short", Tail: 1}, Projects: []Project{}, extra: map[string]any{}}
}

func DataDir() string {
	if xdg := os.Getenv("XDG_DATA_HOME"); xdg != "" {
		return filepath.Join(xdg, "tt")
	}
	if home := os.Getenv("HOME"); home != "" {
		return filepath.Join(home, ".local", "share", "tt")
	}
	return ""
}

func Load() (*Config, error) {
	return load(fsutil.Write)
}

func load(write func(string, []byte, fsutil.Mode) (bool, error)) (*Config, error) {
	if path := os.Getenv("TT_CONFIG"); path != "" {
		return LoadFrom(path)
	}
	data := DataDir()
	if data == "" {
		return defaultConfig(""), nil
	}
	path := filepath.Join(data, "config.toml")
	if _, err := os.Stat(path); err == nil || !errors.Is(err, os.ErrNotExist) {
		return LoadFrom(path)
	}
	legacyDir := os.Getenv("XDG_CONFIG_HOME")
	if legacyDir == "" && os.Getenv("HOME") != "" {
		legacyDir = filepath.Join(os.Getenv("HOME"), ".config")
	}
	if legacyDir == "" {
		return LoadFrom(path)
	}
	legacy := filepath.Join(legacyDir, "tt", "config.toml")
	if info, err := os.Stat(legacy); err != nil || !info.Mode().IsRegular() {
		return LoadFrom(path)
	}
	config, err := LoadFrom(legacy)
	if err != nil {
		return nil, err
	}
	config.Path = path
	text, err := config.serialized()
	if err != nil {
		return nil, err
	}
	created, err := write(path, text, fsutil.CreateNewWithParents)
	if err != nil {
		return nil, fmt.Errorf("config filesystem error at %s: %w", path, err)
	}
	if !created {
		return LoadFrom(path)
	}
	return config, nil
}

func LoadFrom(path string) (*Config, error) {
	config := defaultConfig(path)
	if path == "" {
		return config, nil
	}
	data, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		return config, nil
	}
	if err != nil {
		return nil, fmt.Errorf("config filesystem error at %s: %w", path, err)
	}
	var table map[string]any
	if _, err := toml.Decode(string(data), &table); err != nil {
		return nil, fmt.Errorf("invalid config %s: %w", path, err)
	}
	if table == nil {
		table = map[string]any{}
	}
	invalid := func(key, detail string) error { return fmt.Errorf("invalid %s in %s: %s", key, path, detail) }
	if value, ok := table["capture_target"]; ok {
		text, ok := value.(string)
		if !ok {
			return nil, invalid("capture_target", "expected a string")
		}
		text = strings.TrimSpace(text)
		if err := vault.ValidateID(text); err != nil {
			return nil, fmt.Errorf("invalid capture_target in %s: %w", path, err)
		}
		config.CaptureTarget = &text
		delete(table, "capture_target")
	}
	if value, ok := table["vault"]; ok {
		text, ok := value.(string)
		if !ok {
			return nil, invalid("vault", "expected a string")
		}
		config.DeprecatedVault = &text
		delete(table, "vault")
	}
	if value, ok := table["path_display"]; ok {
		display, ok := value.(map[string]any)
		if !ok {
			return nil, invalid("path_display", "expected a table")
		}
		for key, value := range display {
			switch key {
			case "style":
				style, ok := value.(string)
				if !ok {
					return nil, invalid("path_display.style", "expected a string")
				}
				if style != "home" && style != "short" && style != "tail" {
					return nil, invalid("path_display.style", fmt.Sprintf("unknown style %q; expected home, short, or tail", style))
				}
				config.Display.Style = style
			case "tail":
				tail, ok := value.(int64)
				if !ok || tail < 0 {
					return nil, invalid("path_display.tail", "expected a non-negative integer")
				}
				config.Display.Tail = tail
			default:
				return nil, invalid("path_display", fmt.Sprintf("unknown key %q", key))
			}
		}
		if config.Display.Style == "tail" && config.Display.Tail == 0 {
			return nil, invalid("path_display.tail", "tail must be at least 1")
		}
		delete(table, "path_display")
	}
	if value, ok := table["project"]; ok {
		entries := []any{}
		if _, ok := value.(map[string]any); ok {
			entries = append(entries, value)
		} else if rv := reflect.ValueOf(value); rv.IsValid() && rv.Kind() == reflect.Slice {
			for i := 0; i < rv.Len(); i++ {
				entries = append(entries, rv.Index(i).Interface())
			}
		} else {
			return nil, invalid("project", "expected a table")
		}
		for _, entry := range entries {
			row, ok := entry.(map[string]any)
			if !ok {
				return nil, invalid("project", "expected a table")
			}
			pathValue, exists := row["path"]
			if !exists {
				return nil, invalid("project.path", "missing")
			}
			projectPath, ok := pathValue.(string)
			if !ok {
				return nil, invalid("project.path", "expected a string")
			}
			slugValue, exists := row["slug"]
			if !exists {
				return nil, invalid("project.slug", "missing")
			}
			slug, ok := slugValue.(string)
			if !ok {
				return nil, invalid("project.slug", "expected a string")
			}
			if slug == "" {
				return nil, invalid("project.slug", "must not be empty")
			}
			never := false
			if value, exists := row["never_ask_nested"]; exists {
				var ok bool
				never, ok = value.(bool)
				if !ok {
					return nil, invalid("project.never_ask_nested", "expected a boolean")
				}
			}
			config.Projects = append(config.Projects, Project{Path: expandPath(projectPath), Slug: slug, NeverAskNested: never})
		}
		delete(table, "project")
	}
	config.extra = table
	return config, nil
}

func expandPath(path string) string {
	home := os.Getenv("HOME")
	if home != "" && path == "~" {
		return home
	}
	if home != "" && strings.HasPrefix(path, "~/") {
		return filepath.Join(home, path[2:])
	}
	return path
}

func encodePath(path string) string {
	home := os.Getenv("HOME")
	if home == "" {
		return path
	}
	for _, base := range []string{Normalize(home), home} {
		relative, err := filepath.Rel(base, Normalize(path))
		if err != nil || relative == ".." || strings.HasPrefix(relative, ".."+string(filepath.Separator)) || filepath.IsAbs(relative) {
			relative, err = filepath.Rel(base, path)
		}
		if err == nil && relative != ".." && !strings.HasPrefix(relative, ".."+string(filepath.Separator)) && !filepath.IsAbs(relative) {
			if relative == "." {
				return "~"
			}
			return "~/" + filepath.ToSlash(relative)
		}
	}
	return path
}

func (config *Config) serialized() ([]byte, error) {
	if config.Path == "" {
		return nil, errors.New("config file location is unknown; cannot save")
	}
	table := map[string]any{}
	for key, value := range config.extra {
		table[key] = value
	}
	if config.CaptureTarget != nil {
		table["capture_target"] = *config.CaptureTarget
	}
	if config.DeprecatedVault != nil {
		table["vault"] = *config.DeprecatedVault
	}
	if config.Display != (PathDisplay{Style: "short", Tail: 1}) {
		table["path_display"] = map[string]any{"style": config.Display.Style, "tail": config.Display.Tail}
	}
	if len(config.Projects) > 0 {
		projects := make([]map[string]any, 0, len(config.Projects))
		for _, p := range config.Projects {
			projects = append(projects, map[string]any{"path": encodePath(p.Path), "slug": p.Slug, "never_ask_nested": p.NeverAskNested})
		}
		table["project"] = projects
	}
	var out bytes.Buffer
	if err := toml.NewEncoder(&out).Encode(table); err != nil {
		return nil, fmt.Errorf("cannot serialize config %s: %w", config.Path, err)
	}
	return out.Bytes(), nil
}

func (config *Config) Save() error {
	data, err := config.serialized()
	if err != nil {
		return err
	}
	if _, err := fsutil.Write(config.Path, data, fsutil.ReplaceWithParents); err != nil {
		return fmt.Errorf("config filesystem error at %s: %w", config.Path, err)
	}
	return nil
}
