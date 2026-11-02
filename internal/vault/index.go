package vault

import (
	"fmt"
	"path/filepath"
	"sort"
	"strings"
	"time"
	"unicode"
)

// Issue describes a file or parent edge that could not be loaded normally.
type Issue struct {
	Path   string
	Kind   string
	Detail string
}

func (issue Issue) String() string {
	return fmt.Sprintf("%s (%s): %s", issue.Path, issue.Kind, issue.Detail)
}

type index struct {
	roots     []string
	parents   map[string]string
	children  map[string][]string
	links     map[string][]string
	backlinks map[string][]string
}

func sortedIDs(tasks map[string]Task) []string {
	ids := make([]string, 0, len(tasks))
	for id := range tasks {
		ids = append(ids, id)
	}
	sort.Strings(ids)
	return ids
}

// lowercaseTitle follows Unicode full lowercase mappings used by Rust's
// String::to_lowercase, including dotted I and context-sensitive Greek sigma.
func lowercaseTitle(title string) string {
	if !strings.ContainsAny(title, "İΣ") {
		return strings.ToLower(title)
	}
	runes := []rune(title)
	var out strings.Builder
	out.Grow(len(title))
	for i, r := range runes {
		switch r {
		case 'İ':
			out.WriteString("i\u0307")
		case 'Σ':
			if finalSigma(runes, i) {
				out.WriteRune('ς')
			} else {
				out.WriteRune('σ')
			}
		default:
			out.WriteRune(unicode.ToLower(r))
		}
	}
	return out.String()
}

func isCased(r rune) bool {
	return unicode.IsUpper(r) || unicode.IsLower(r) || unicode.IsTitle(r) ||
		unicode.Is(unicode.Other_Uppercase, r) || unicode.Is(unicode.Other_Lowercase, r)
}

func caseIgnorable(r rune) bool {
	if unicode.Is(unicode.Mn, r) || unicode.Is(unicode.Me, r) || unicode.Is(unicode.Cf, r) || unicode.Is(unicode.Lm, r) || unicode.Is(unicode.Sk, r) {
		return true
	}
	switch r {
	case '\'', '.', ':', '\u00b7', '\u0387', '\u055f', '\u05f4', '\u2018', '\u2019', '\u2024', '\u2027', '\ufe13', '\ufe52', '\ufe55', '\uff07', '\uff0e', '\uff1a':
		return true
	}
	return false
}

func finalSigma(runes []rune, index int) bool {
	preceded := false
	for i := index - 1; i >= 0; i-- {
		if isCased(runes[i]) {
			preceded = true
			break
		}
		if !caseIgnorable(runes[i]) {
			break
		}
	}
	if !preceded {
		return false
	}
	for i := index + 1; i < len(runes); i++ {
		if isCased(runes[i]) {
			return false
		}
		if !caseIgnorable(runes[i]) {
			break
		}
	}
	return true
}

func displayLess(a, b Task) bool {
	switch {
	case a.Rank != nil && b.Rank == nil:
		return true
	case a.Rank == nil && b.Rank != nil:
		return false
	case a.Rank != nil && b.Rank != nil && *a.Rank != *b.Rank:
		return *a.Rank < *b.Rank
	}
	at, bt := lowercaseTitle(a.Title), lowercaseTitle(b.Title)
	if at != bt {
		return at < bt
	}
	return a.ID < b.ID
}

func buildIndex(tasks map[string]Task, root string) (index, []Issue) {
	idx := index{parents: map[string]string{}, children: map[string][]string{}, links: map[string][]string{}, backlinks: map[string][]string{}}
	issues := []Issue{}
	ids := sortedIDs(tasks)
	issue := func(id, kind, detail string) {
		issues = append(issues, Issue{Path: filepath.Join(root, id+".md"), Kind: kind, Detail: detail})
	}
	for _, id := range ids {
		task := tasks[id]
		if task.Parent == nil {
			continue
		}
		parent := *task.Parent
		switch {
		case parent == id:
			issue(id, "cycle", fmt.Sprintf("parent cycle %s → %s; this edge removed and the task treated as root", id, id))
		case tasks[parent].ID != "":
			idx.parents[id] = parent
		default:
			issue(id, "dangling parent", fmt.Sprintf("parent %s not found; task %s treated as root", parent, id))
		}
	}
	safe := map[string]bool{}
	for _, start := range ids {
		if safe[start] {
			continue
		}
		walk := []string{}
		onWalk := map[string]int{}
		for current := start; ; {
			if safe[current] {
				break
			}
			if pos, repeated := onWalk[current]; repeated {
				delete(idx.parents, current)
				cycle := append(append([]string{}, walk[pos:]...), current)
				issue(current, "cycle", fmt.Sprintf("parent cycle %s; this edge removed and the task treated as root", strings.Join(cycle, " → ")))
				break
			}
			onWalk[current] = len(walk)
			walk = append(walk, current)
			parent, ok := idx.parents[current]
			if !ok {
				break
			}
			current = parent
		}
		for _, id := range walk {
			safe[id] = true
		}
	}
	for _, id := range ids {
		if parent, ok := idx.parents[id]; ok {
			idx.children[parent] = append(idx.children[parent], id)
		} else {
			idx.roots = append(idx.roots, id)
		}
		for _, target := range extractLinks(tasks[id].Body) {
			idx.links[id] = append(idx.links[id], target)
			idx.backlinks[target] = append(idx.backlinks[target], id)
		}
	}
	sortGroup := func(group []string) {
		sort.Slice(group, func(i, j int) bool { return displayLess(tasks[group[i]], tasks[group[j]]) })
	}
	sortGroup(idx.roots)
	for _, group := range idx.children {
		sortGroup(group)
	}
	return idx, issues
}

type wikiLink struct {
	target     string
	alias      string
	aliasStart int
	aliasEnd   int
	hasAlias   bool
}

func linkTarget(target string) (string, bool) {
	target = strings.TrimSpace(target)
	target = strings.TrimSuffix(target, ".md")
	if i := strings.LastIndexAny(target, "/\\"); i >= 0 {
		target = target[i+1:]
	}
	return target, ValidateID(target) == nil
}

func parseWikilinks(body string) []wikiLink {
	links := []wikiLink{}
	fenced := false
	for lineStart := 0; lineStart < len(body); {
		lineEnd := strings.IndexByte(body[lineStart:], '\n')
		if lineEnd < 0 {
			lineEnd = len(body)
		} else {
			lineEnd += lineStart
		}
		end := lineEnd
		if end > lineStart && body[end-1] == '\r' {
			end--
		}
		line := body[lineStart:end]
		trimmed := strings.TrimLeftFunc(line, unicode.IsSpace)
		if strings.HasPrefix(trimmed, "```") || strings.HasPrefix(trimmed, "~~~") {
			fenced = !fenced
		} else if !fenced {
			for i := 0; i < len(line); {
				if line[i] == '`' {
					close := strings.IndexByte(line[i+1:], '`')
					if close < 0 {
						break
					}
					i += close + 2
					continue
				}
				if strings.HasPrefix(line[i:], "[[") {
					close := strings.Index(line[i+2:], "]]")
					if close < 0 {
						break
					}
					close += i + 2
					inner := line[i+2 : close]
					target, alias, hasAlias := strings.Cut(inner, "|")
					if target, valid := linkTarget(target); valid {
						link := wikiLink{target: target, alias: alias, hasAlias: hasAlias}
						if hasAlias {
							link.aliasStart = lineStart + close - len(alias)
							link.aliasEnd = lineStart + close
						}
						links = append(links, link)
					}
					i = close + 2
					continue
				}
				i++
			}
		}
		if lineEnd == len(body) {
			break
		}
		lineStart = lineEnd + 1
	}
	return links
}

func extractLinks(body string) []string {
	out := []string{}
	seen := map[string]bool{}
	for _, link := range parseWikilinks(body) {
		if !seen[link.target] {
			out = append(out, link.target)
			seen[link.target] = true
		}
	}
	return out
}

type titleChange struct{ old, new string }

func rewriteMirrors(body string, changes map[string]titleChange) (string, bool) {
	links := parseWikilinks(body)
	changed := false
	for i := len(links) - 1; i >= 0; i-- {
		link := links[i]
		change, ok := changes[link.target]
		if ok && link.hasAlias && strings.TrimSpace(link.alias) == change.old && link.alias != change.new {
			body = body[:link.aliasStart] + change.new + body[link.aliasEnd:]
			changed = true
		}
	}
	return body, changed
}

// Filter selects tasks; a nil field leaves that dimension unrestricted.
type Filter struct {
	Tag      *string
	State    *string
	DueToday bool
}

func matches(task Task, filter Filter, today time.Time) bool {
	if filter.State != nil && task.State != *filter.State {
		return false
	}
	if filter.DueToday && (task.Due == nil || *task.Due > today.Format("2006-01-02")) {
		return false
	}
	if filter.Tag != nil {
		query := strings.TrimSpace(strings.TrimLeft(strings.TrimSpace(*filter.Tag), "#"))
		if query != "" {
			for _, tag := range task.Tags {
				if tag == query || strings.HasPrefix(tag, query+"/") {
					return true
				}
			}
			return false
		}
	}
	return true
}
