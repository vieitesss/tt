package vault

import (
	"bytes"
	"errors"
	"fmt"
	"os"
	"sort"
	"strings"

	"github.com/vieitesss/tt/internal/fsutil"
)

// DesiredTask is one node in the complete desired project tree.
type DesiredTask struct {
	ID       *string       `json:"id,omitempty"`
	Title    string        `json:"title"`
	State    *string       `json:"state,omitempty"`
	Tags     []string      `json:"tags"`
	Due      *string       `json:"due"`
	Priority *string       `json:"priority"`
	Children []DesiredTask `json:"children"`
}

type ApplyInput struct {
	Revision string        `json:"revision"`
	Tasks    []DesiredTask `json:"tasks"`
}

type ApplyAdd struct {
	ID     string  `json:"id"`
	Title  string  `json:"title"`
	Parent *string `json:"parent"`
}

type ApplyUpdate struct {
	ID     string   `json:"id"`
	Fields []string `json:"fields"`
}

type ApplyMove struct {
	ID   string  `json:"id"`
	From *string `json:"from"`
	To   *string `json:"to"`
}

type ApplyDelete struct {
	ID      string `json:"id"`
	Title   string `json:"title"`
	HasBody bool   `json:"has_body"`
}

type ApplyPlan struct {
	Revision string        `json:"revision"`
	DryRun   bool          `json:"dry_run"`
	Adds     []ApplyAdd    `json:"adds"`
	Updates  []ApplyUpdate `json:"updates"`
	Moves    []ApplyMove   `json:"moves"`
	Reorders int           `json:"reorders"`
	Deletes  []ApplyDelete `json:"deletes"`
}

// PartialApplyError reports the subset already committed after an I/O failure.
type PartialApplyError struct {
	Cause   error
	Applied ApplyPlan
}

func (err *PartialApplyError) Error() string { return err.Cause.Error() }
func (err *PartialApplyError) Unwrap() error { return err.Cause }

func emptyPlan(revision string, dryRun bool) ApplyPlan {
	return ApplyPlan{Revision: revision, DryRun: dryRun, Adds: []ApplyAdd{}, Updates: []ApplyUpdate{}, Moves: []ApplyMove{}, Deletes: []ApplyDelete{}}
}

type preparedTask struct {
	task        Task
	parent      *string
	oldParent   *string
	isNew       bool
	fields      []string
	moved       bool
	rankChanged bool
}

func appendUpdate(plan *ApplyPlan, id string, fields ...string) {
	if len(fields) == 0 {
		return
	}
	for i := range plan.Updates {
		if plan.Updates[i].ID == id {
			plan.Updates[i].Fields = append(plan.Updates[i].Fields, fields...)
			return
		}
	}
	plan.Updates = append(plan.Updates, ApplyUpdate{ID: id, Fields: append([]string{}, fields...)})
}

// Apply validates and plans the complete tree before any writes. Adds precede
// existing-task writes, alias synchronization, and exact-id deletions.
func (v *Vault) Apply(input ApplyInput, dryRun bool) (ApplyPlan, error) {
	// Refresh without title sync: planning, including dry runs, must not write.
	v.scan()
	revision := v.SnapshotRevision()
	if input.Revision != revision {
		return ApplyPlan{}, errors.New("task list changed on disk; reload")
	}
	seen := map[string]bool{}
	var validate func([]DesiredTask) error
	validate = func(nodes []DesiredTask) error {
		for _, node := range nodes {
			if strings.TrimSpace(node.Title) == "" {
				return errors.New("task title must not be empty")
			}
			if node.ID != nil {
				id := *node.ID
				if err := ValidateID(id); err != nil {
					return fmt.Errorf("invalid task id: %s: %w", id, err)
				}
				if seen[id] {
					return fmt.Errorf("duplicate task id: %s", id)
				}
				seen[id] = true
				if _, ok := v.tasks[id]; !ok {
					return fmt.Errorf("task not found: %s", id)
				}
			}
			if node.State != nil && !ValidState(*node.State) {
				return fmt.Errorf("invalid state: %s", *node.State)
			}
			if node.Priority != nil && !ValidPriority(*node.Priority) {
				return fmt.Errorf("invalid priority: %s", *node.Priority)
			}
			if node.Due != nil {
				if err := ValidateDate(*node.Due); err != nil {
					return err
				}
			}
			if err := validate(node.Children); err != nil {
				return err
			}
		}
		return nil
	}
	if err := validate(input.Tasks); err != nil {
		return ApplyPlan{}, err
	}

	plan := emptyPlan(revision, dryRun)
	prepared := map[string]*preparedTask{}
	order := []string{}
	groups := map[string][]string{}
	changes := map[string]titleChange{}
	var prepare func([]DesiredTask, *string) error
	prepare = func(nodes []DesiredTask, parent *string) error {
		for _, node := range nodes {
			id := ""
			isNew := node.ID == nil
			if isNew {
				var err error
				id, err = generateID(func(candidate string) bool {
					_, loaded := v.tasks[candidate]
					_, alreadyPrepared := prepared[candidate]
					_, err := os.Lstat(v.Path(candidate))
					return loaded || alreadyPrepared || err == nil
				})
				if err != nil {
					return err
				}
			} else {
				id = *node.ID
			}
			state := "open"
			if node.State != nil {
				state = *node.State
			}
			tags := NormalizeTags(node.Tags)
			item := &preparedTask{parent: parent, isNew: isNew, fields: []string{}}
			if isNew {
				item.task = NewTask(id, node.Title)
				item.task.Parent = parent
			} else {
				item.task = v.tasks[id]
				item.oldParent = v.Parent(id)
				item.moved = !equalOptional(item.oldParent, parent)
				if item.moved {
					item.task.Parent = parent
				}
				if item.task.Title != node.Title {
					item.fields = append(item.fields, "title")
					changes[id] = titleChange{item.task.Title, node.Title}
				}
				if item.task.State != state {
					item.fields = append(item.fields, "state")
				}
				if !equalStrings(item.task.Tags, tags) {
					item.fields = append(item.fields, "tags")
				}
				if !equalOptional(item.task.Due, node.Due) {
					item.fields = append(item.fields, "due")
				}
				if !equalOptional(item.task.Priority, node.Priority) {
					item.fields = append(item.fields, "priority")
				}
			}
			item.task.Title, item.task.State, item.task.Tags, item.task.Due, item.task.Priority = node.Title, state, tags, node.Due, node.Priority
			prepared[id] = item
			order = append(order, id)
			key := ""
			if parent != nil {
				key = *parent
			}
			groups[key] = append(groups[key], id)
			childParent := id
			if err := prepare(node.Children, &childParent); err != nil {
				return err
			}
		}
		return nil
	}
	if err := prepare(input.Tasks, nil); err != nil {
		return ApplyPlan{}, err
	}
	for _, desired := range groups {
		sorted := append([]string{}, desired...)
		sort.Slice(sorted, func(i, j int) bool { return displayLess(prepared[sorted[i]].task, prepared[sorted[j]].task) })
		if equalStrings(sorted, desired) {
			continue
		}
		for i, id := range desired {
			item := prepared[id]
			rank := int32(i)
			if item.task.Rank == nil || *item.task.Rank != rank {
				item.task.Rank = &rank
				item.rankChanged = true
				plan.Reorders++
			}
		}
	}
	for _, id := range order {
		item := prepared[id]
		if item.isNew {
			plan.Adds = append(plan.Adds, ApplyAdd{ID: id, Title: item.task.Title, Parent: item.parent})
		} else {
			appendUpdate(&plan, id, item.fields...)
			if item.moved {
				plan.Moves = append(plan.Moves, ApplyMove{ID: id, From: item.oldParent, To: item.parent})
			}
		}
		if _, changed := rewriteMirrors(item.task.Body, changes); changed {
			appendUpdate(&plan, id, "body")
		}
	}
	deletedBytes := map[string][]byte{}
	for _, id := range sortedIDs(v.tasks) {
		if _, retained := prepared[id]; retained {
			continue
		}
		task := v.tasks[id]
		plan.Deletes = append(plan.Deletes, ApplyDelete{ID: id, Title: task.Title, HasBody: len(task.Body) > 0})
		data, err := os.ReadFile(v.Path(id))
		if err != nil {
			return ApplyPlan{}, fsError(v.Path(id), err)
		}
		deletedBytes[id] = data
	}
	if dryRun {
		return plan, nil
	}
	// Check again after planning, before the first write.
	if current, err := v.Revision(); err != nil {
		return ApplyPlan{}, err
	} else if current != revision {
		return ApplyPlan{}, errors.New("task list changed on disk; reload")
	}
	applied := emptyPlan(revision, false)
	partial := func(err error) (ApplyPlan, error) {
		if current, revisionErr := v.Revision(); revisionErr == nil {
			applied.Revision = current
		}
		return ApplyPlan{}, &PartialApplyError{Cause: err, Applied: applied}
	}
	for _, id := range order {
		item := prepared[id]
		if !item.isNew {
			continue
		}
		if err := v.persist(item.task, fsutil.CreateNew); err != nil {
			return partial(err)
		}
		applied.Adds = append(applied.Adds, ApplyAdd{ID: id, Title: item.task.Title, Parent: item.parent})
		if item.rankChanged {
			applied.Reorders++
		}
	}
	for _, id := range order {
		item := prepared[id]
		if item.isNew || len(item.fields) == 0 && !item.moved && !item.rankChanged {
			continue
		}
		if err := v.persist(item.task, fsutil.Replace); err != nil {
			return partial(err)
		}
		appendUpdate(&applied, id, item.fields...)
		if item.moved {
			applied.Moves = append(applied.Moves, ApplyMove{ID: id, From: item.oldParent, To: item.parent})
		}
		if item.rankChanged {
			applied.Reorders++
		}
	}
	// Alias rewrites are calculated from the original title diffs in one pass.
	ids := make([]string, 0, len(prepared))
	for id := range prepared {
		ids = append(ids, id)
	}
	sort.Strings(ids)
	for _, id := range ids {
		task := v.tasks[id]
		body, changed := rewriteMirrors(task.Body, changes)
		if !changed {
			continue
		}
		task.Body = body
		if err := v.persist(task, fsutil.Replace); err != nil {
			return partial(err)
		}
		appendUpdate(&applied, id, "body")
	}
	for _, deletion := range plan.Deletes {
		path := v.Path(deletion.ID)
		current, err := os.ReadFile(path)
		if err != nil && !errors.Is(err, os.ErrNotExist) {
			return partial(fsError(path, err))
		}
		if err == nil && !bytes.Equal(current, deletedBytes[deletion.ID]) {
			return partial(fmt.Errorf("task file changed during apply; kept %s", path))
		}
		if err == nil {
			if err := v.removeFile(path); err != nil && !errors.Is(err, os.ErrNotExist) {
				return partial(fsError(path, err))
			}
		}
		delete(v.tasks, deletion.ID)
		v.rebuild()
		applied.Deletes = append(applied.Deletes, deletion)
	}
	newRevision, err := v.Revision()
	if err != nil {
		return partial(err)
	}
	plan.Revision = newRevision
	return plan, nil
}

func equalStrings(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}
