package vault

import (
	"fmt"
	"strings"

	"github.com/vieitesss/tt/internal/fsutil"
)

func (v *Vault) require(id string) (Task, error) {
	task, ok := v.tasks[id]
	if !ok {
		return Task{}, fmt.Errorf("task not found: %s", id)
	}
	return task, nil
}

func (v *Vault) SetState(id, state string) (Task, error) {
	task, err := v.require(id)
	if err != nil {
		return Task{}, err
	}
	if !ValidState(state) {
		return Task{}, fmt.Errorf("invalid state: %s", state)
	}
	task.State = state
	if err := v.persist(task, fsutil.Replace); err != nil {
		return Task{}, err
	}
	return task, nil
}

// SetTitle writes the title, then updates mirror aliases in this Store.
func (v *Vault) SetTitle(id, title string) (Task, int, error) {
	if strings.TrimSpace(title) == "" {
		return Task{}, 0, fmt.Errorf("task title must not be empty")
	}
	task, err := v.require(id)
	if err != nil {
		return Task{}, 0, err
	}
	old := task.Title
	task.Title = title
	if err := v.persist(task, fsutil.Replace); err != nil {
		return Task{}, 0, err
	}
	changes := map[string]titleChange{}
	if old != title {
		changes[id] = titleChange{old, title}
	}
	count, path, err := v.syncMirrors(changes)
	if err != nil {
		return Task{}, count, fmt.Errorf("partial commit (title_committed=true, reload_committed=false); %d mirror file(s) updated before failure at %s: %w", count, path, err)
	}
	return v.tasks[id], count, nil
}

// SetParent appends a task to its new sibling group and closes the old rank gap.
func (v *Vault) SetParent(id string, parent *string) (Task, error) {
	task, err := v.require(id)
	if err != nil {
		return Task{}, err
	}
	if parent != nil {
		for current := *parent; current != ""; {
			if current == id {
				return Task{}, fmt.Errorf("invalid parent %s for task %s: a task cannot parent itself or a descendant", *parent, id)
			}
			current = v.index.parents[current]
		}
		if _, ok := v.tasks[*parent]; !ok {
			return Task{}, fmt.Errorf("task not found: %s", *parent)
		}
	}
	oldParent := v.Parent(id)
	oldSiblings := v.destinationSiblings(oldParent)
	hadRanks := false
	for _, sibling := range oldSiblings {
		if v.tasks[sibling].Rank != nil {
			hadRanks = true
		}
	}
	sameGroup := equalOptional(oldParent, parent)
	destination := v.destinationSiblings(parent)
	filtered := destination[:0]
	for _, sibling := range destination {
		if sibling != id {
			filtered = append(filtered, sibling)
		}
	}
	destination = append(filtered, id)
	task.Parent = parent
	for i, sibling := range destination {
		target := v.tasks[sibling]
		if sibling == id {
			target = task
		}
		rank := int32(i)
		target.Rank = &rank
		if err := v.persist(target, fsutil.Replace); err != nil {
			return Task{}, err
		}
	}
	if !sameGroup && hadRanks {
		rank := int32(0)
		for _, sibling := range oldSiblings {
			if sibling == id {
				continue
			}
			target := v.tasks[sibling]
			siblingRank := rank
			target.Rank = &siblingRank
			if err := v.persist(target, fsutil.Replace); err != nil {
				return Task{}, err
			}
			rank++
		}
	}
	return v.tasks[id], nil
}

func equalOptional[T comparable](a, b *T) bool {
	if a == nil || b == nil {
		return a == nil && b == nil
	}
	return *a == *b
}

func (v *Vault) syncMirrors(changes map[string]titleChange) (int, string, error) {
	count := 0
	for _, id := range sortedIDs(v.tasks) {
		task := v.tasks[id]
		body, changed := rewriteMirrors(task.Body, changes)
		if !changed {
			continue
		}
		task.Body = body
		if err := v.persist(task, fsutil.Replace); err != nil {
			return count, v.Path(id), err
		}
		count++
	}
	return count, "", nil
}
