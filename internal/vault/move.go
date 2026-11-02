package vault

import (
	"bytes"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"

	"github.com/vieitesss/tt/internal/fsutil"
)

func (v *Vault) closure(seeds []string) []string {
	seen := map[string]bool{}
	stack := append([]string{}, seeds...)
	for len(stack) > 0 {
		id := stack[len(stack)-1]
		stack = stack[:len(stack)-1]
		if seen[id] {
			continue
		}
		if _, ok := v.tasks[id]; !ok {
			continue
		}
		seen[id] = true
		stack = append(stack, v.index.children[id]...)
	}
	out := make([]string, 0, len(seen))
	for id := range seen {
		out = append(out, id)
	}
	sort.Strings(out)
	return out
}

// Subtree returns one task and its descendants in identity order.
func (v *Vault) Subtree(id string) []Task {
	ids := v.closure([]string{id})
	out := make([]Task, 0, len(ids))
	for _, id := range ids {
		out = append(out, v.tasks[id])
	}
	return out
}

// MoveTo copies every requested subtree to target before removing any source.
// A source that changes after copying is kept, and progress is reported.
func (v *Vault) MoveTo(target *Vault, ids []string, parent *string) ([]Task, error) {
	sourceRoot, sourceErr := filepath.EvalSymlinks(v.root)
	targetRoot, targetErr := filepath.EvalSymlinks(target.root)
	if v.root == target.root || sourceErr == nil && targetErr == nil && sourceRoot == targetRoot {
		return nil, errors.New("source and target are the same store")
	}
	requested := map[string]bool{}
	for _, id := range ids {
		if _, err := v.require(id); err != nil {
			return nil, err
		}
		requested[id] = true
	}
	if len(requested) == 0 {
		return []Task{}, nil
	}
	if parent != nil {
		if _, err := target.require(*parent); err != nil {
			return nil, err
		}
	}
	roots := map[string]bool{}
	for id := range requested {
		covered := false
		for current := v.index.parents[id]; current != ""; current = v.index.parents[current] {
			if requested[current] {
				covered = true
				break
			}
		}
		if !covered {
			roots[id] = true
		}
	}
	rootIDs := make([]string, 0, len(roots))
	for id := range roots {
		rootIDs = append(rootIDs, id)
	}
	movedIDs := v.closure(rootIDs)
	for _, id := range movedIDs {
		_, loaded := target.tasks[id]
		_, err := os.Stat(target.Path(id))
		if loaded || err == nil {
			return nil, fmt.Errorf("task id %s already exists in the target store (target_written=0)", id)
		}
	}
	type source struct {
		id   string
		data []byte
		task Task
	}
	sources := make([]source, 0, len(movedIDs))
	moved := make([]Task, 0, len(movedIDs))
	for _, id := range movedIDs {
		path := v.Path(id)
		data, err := os.ReadFile(path)
		if err != nil {
			return nil, fsError(path, err)
		}
		task, err := ParseDocument(string(data))
		if err != nil {
			return nil, fmt.Errorf("cannot move task file at %s: %w", path, err)
		}
		task.ID = id
		if roots[id] {
			task.Parent = parent
			task.Rank = nil
		}
		sources = append(sources, source{id, data, task})
		moved = append(moved, task)
	}
	written, removed := 0, 0
	partial := func(path string, err error) error {
		return fmt.Errorf("partial move (target_written=%d, source_removed=%d) at %s: %w", written, removed, path, err)
	}
	for _, item := range sources {
		path := target.Path(item.id)
		document, err := item.task.Document()
		if err != nil {
			return nil, partial(path, err)
		}
		created, err := target.writeFile(path, []byte(document), fsutil.CreateNew)
		if err != nil {
			return nil, partial(path, err)
		}
		if !created {
			return nil, fmt.Errorf("task id %s already exists in the target store (target_written=%d)", item.id, written)
		}
		target.tasks[item.id] = item.task
		target.rebuild()
		written++
	}
	for _, item := range sources {
		path := v.Path(item.id)
		current, err := os.ReadFile(path)
		if err == nil && !bytes.Equal(current, item.data) {
			err = errors.New("source changed after target copy; source was kept")
		}
		if err == nil {
			err = v.removeFile(path)
		}
		if err != nil {
			v.scan()
			return nil, partial(path, err)
		}
		delete(v.tasks, item.id)
		removed++
	}
	v.rebuild()
	return moved, nil
}
