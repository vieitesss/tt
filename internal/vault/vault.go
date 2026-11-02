// Package vault owns the markdown task Store and its derived tree and links.
package vault

import (
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/vieitesss/tt/internal/fsutil"
)

// Vault is a cache over a single Store. All mutations persist before changing it.
type Vault struct {
	root             string
	tasks            map[string]Task
	index            index
	scanIssues       []Issue
	issues           []Issue
	snapshotRevision string
	readFile         func(string) ([]byte, error)
	writeFile        func(string, []byte, fsutil.Mode) (bool, error)
	removeFile       func(string) error
}

func Open(root string) (*Vault, error) {
	if err := os.MkdirAll(root, 0o777); err != nil {
		return nil, fsError(root, err)
	}
	v := &Vault{root: root, tasks: map[string]Task{}, readFile: os.ReadFile, writeFile: fsutil.Write, removeFile: os.Remove}
	v.scan()
	return v, nil
}

func fsError(path string, err error) error {
	var pathErr *os.PathError
	if errors.As(err, &pathErr) {
		err = pathErr.Err
	}
	return fmt.Errorf("filesystem error at %s: %w", path, err)
}

func (v *Vault) Root() string                 { return v.root }
func (v *Vault) Path(id string) string        { return filepath.Join(v.root, id+".md") }
func (v *Vault) Issues() []Issue              { return append([]Issue{}, v.issues...) }
func (v *Vault) Get(id string) (Task, bool)   { task, ok := v.tasks[id]; return task, ok }
func (v *Vault) Roots() []string              { return append([]string{}, v.index.roots...) }
func (v *Vault) Children(id string) []string  { return append([]string{}, v.index.children[id]...) }
func (v *Vault) Links(id string) []string     { return append([]string{}, v.index.links[id]...) }
func (v *Vault) Backlinks(id string) []string { return append([]string{}, v.index.backlinks[id]...) }
func (v *Vault) Parent(id string) *string {
	if parent, ok := v.index.parents[id]; ok {
		return &parent
	}
	return nil
}

func (v *Vault) Tasks() []Task {
	out := make([]Task, 0, len(v.tasks))
	for _, id := range sortedIDs(v.tasks) {
		out = append(out, v.tasks[id])
	}
	return out
}

func (v *Vault) scan() {
	v.tasks = map[string]Task{}
	v.scanIssues = []Issue{}
	records := map[string][]byte{}
	entries, err := os.ReadDir(v.root)
	if err != nil {
		v.scanIssues = append(v.scanIssues, Issue{v.root, "unreadable", "cannot read vault folder: " + err.Error()})
		v.snapshotRevision = hashRecords(records)
		v.rebuild()
		return
	}
	for _, entry := range entries {
		path := filepath.Join(v.root, entry.Name())
		if filepath.Ext(entry.Name()) != ".md" {
			continue
		}
		info, err := os.Stat(path)
		if err != nil || !info.Mode().IsRegular() {
			continue
		}
		data, err := v.readFile(path)
		if err != nil {
			v.scanIssues = append(v.scanIssues, Issue{path, "unreadable", "cannot read file: " + err.Error()})
			continue
		}
		if !utf8.Valid(data) {
			v.scanIssues = append(v.scanIssues, Issue{path, "malformed", "file is not valid UTF-8"})
			continue
		}
		task, err := ParseDocument(string(data))
		if errors.Is(err, ErrMissingFrontmatter) || errors.Is(err, ErrForeignFrontmatter) {
			continue
		}
		if err != nil {
			v.scanIssues = append(v.scanIssues, Issue{path, "malformed", err.Error()})
			continue
		}
		stem := strings.TrimSuffix(entry.Name(), ".md")
		if ValidateID(stem) != nil {
			v.scanIssues = append(v.scanIssues, Issue{path, "malformed", fmt.Sprintf("file name %q is not a valid task id", stem)})
			continue
		}
		if stem != task.DeclaredID {
			v.scanIssues = append(v.scanIssues, Issue{path, "id mismatch", fmt.Sprintf("frontmatter id %s does not match the file name %s", task.DeclaredID, stem)})
		}
		task.ID = stem
		v.tasks[stem] = task
		records[stem] = data
	}
	v.snapshotRevision = hashRecords(records)
	v.rebuild()
}

func (v *Vault) rebuild() {
	idx, indexIssues := buildIndex(v.tasks, v.root)
	v.index = idx
	v.issues = append(append([]Issue{}, v.scanIssues...), indexIssues...)
}

// Reload observes external edits and synchronizes aliases for observed renames.
func (v *Vault) Reload() error {
	oldTitles := map[string]string{}
	for id, task := range v.tasks {
		oldTitles[id] = task.Title
	}
	v.scan()
	changes := map[string]titleChange{}
	for id, task := range v.tasks {
		if old, ok := oldTitles[id]; ok && old != task.Title {
			changes[id] = titleChange{old, task.Title}
		}
	}
	count, path, err := v.syncMirrors(changes)
	if err != nil {
		return fmt.Errorf("partial commit (title_committed=false, reload_committed=true); %d mirror file(s) updated before failure at %s: %w", count, path, err)
	}
	return nil
}

// SnapshotRevision identifies the file bytes parsed by the last scan.
func (v *Vault) SnapshotRevision() string { return v.snapshotRevision }

func hashRecords(records map[string][]byte) string {
	ids := make([]string, 0, len(records))
	for id := range records {
		ids = append(ids, id)
	}
	sort.Strings(ids)
	hash := sha256.New()
	hash.Write([]byte("tt-store-revision-v1"))
	var frame [8]byte
	writeLength := func(n int) {
		binary.BigEndian.PutUint64(frame[:], uint64(n))
		hash.Write(frame[:])
	}
	writeLength(len(ids))
	for _, id := range ids {
		writeLength(len(id))
		hash.Write([]byte(id))
		writeLength(len(records[id]))
		hash.Write(records[id])
	}
	return hex.EncodeToString(hash.Sum(nil))
}

// Revision hashes the current Store, including tasks added since the last scan.
func (v *Vault) Revision() (string, error) {
	current := &Vault{root: v.root, readFile: os.ReadFile}
	current.scan()
	for _, issue := range current.scanIssues {
		if issue.Kind == "unreadable" {
			return "", fsError(issue.Path, errors.New(issue.Detail))
		}
	}
	return current.SnapshotRevision(), nil
}

// AddInput describes a new task and its position among siblings.
type AddInput struct {
	Title    string
	Parent   *string
	Tags     []string
	Due      *string
	Priority *string
	Body     string
}

func (v *Vault) Add(input AddInput) (Task, error) {
	if strings.TrimSpace(input.Title) == "" {
		return Task{}, errors.New("task title must not be empty")
	}
	if !utf8.ValidString(input.Body) {
		return Task{}, errors.New("task body is not valid UTF-8")
	}
	if input.Parent != nil {
		if err := ValidateID(*input.Parent); err != nil {
			return Task{}, fmt.Errorf("invalid parent id: %w", err)
		}
	}
	if input.Due != nil {
		if err := ValidateDate(*input.Due); err != nil {
			return Task{}, err
		}
	}
	if input.Priority != nil && !ValidPriority(*input.Priority) {
		return Task{}, fmt.Errorf("invalid priority: %s", *input.Priority)
	}
	id, err := generateID(func(id string) bool {
		_, loaded := v.tasks[id]
		_, err := os.Lstat(v.Path(id))
		return loaded || err == nil
	})
	if err != nil {
		return Task{}, err
	}
	task := NewTask(id, input.Title)
	task.Parent, task.Tags, task.Due, task.Priority, task.Body = input.Parent, NormalizeTags(input.Tags), input.Due, input.Priority, input.Body
	siblings := v.destinationSiblings(task.Parent)
	for i, id := range siblings {
		sibling := v.tasks[id]
		rank := int32(i)
		sibling.Rank = &rank
		if err := v.persist(sibling, fsutil.Replace); err != nil {
			return Task{}, err
		}
	}
	rank := int32(len(siblings))
	task.Rank = &rank
	if err := v.persist(task, fsutil.CreateNew); err != nil {
		return Task{}, err
	}
	return task, nil
}

func (v *Vault) destinationSiblings(parent *string) []string {
	if parent != nil {
		if _, ok := v.tasks[*parent]; ok {
			return v.Children(*parent)
		}
	}
	return v.Roots()
}

func (v *Vault) persist(task Task, mode fsutil.Mode) error {
	path := v.Path(task.ID)
	document, err := task.Document()
	if err != nil {
		return err
	}
	if !utf8.ValidString(document) {
		return errors.New("task document is not valid UTF-8")
	}
	created, err := v.writeFile(path, []byte(document), mode)
	if err != nil {
		return fsError(path, err)
	}
	if !created {
		return fmt.Errorf("task id %s already exists", task.ID)
	}
	v.tasks[task.ID] = task
	v.rebuild()
	return nil
}

// Filtered returns matching tasks in identity order.
func (v *Vault) Filtered(filter Filter, today time.Time) []Task {
	out := []Task{}
	for _, task := range v.Tasks() {
		if matches(task, filter, today) {
			out = append(out, task)
		}
	}
	return out
}

type Node struct {
	Task     Task
	Children []Node
}

// Tree omits nonmatching tasks, promoting their matching children to roots.
func (v *Vault) Tree(filter Filter, today time.Time) []Node {
	matching := map[string]bool{}
	for _, task := range v.Filtered(filter, today) {
		matching[task.ID] = true
	}
	roots := []string{}
	for id := range matching {
		if parent, ok := v.index.parents[id]; !ok || !matching[parent] {
			roots = append(roots, id)
		}
	}
	sort.Slice(roots, func(i, j int) bool { return displayLess(v.tasks[roots[i]], v.tasks[roots[j]]) })
	var subtree func(string) Node
	subtree = func(id string) Node {
		node := Node{Task: v.tasks[id], Children: []Node{}}
		for _, child := range v.index.children[id] {
			if matching[child] {
				node.Children = append(node.Children, subtree(child))
			}
		}
		return node
	}
	out := []Node{}
	for _, id := range roots {
		out = append(out, subtree(id))
	}
	return out
}
