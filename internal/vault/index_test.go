package vault

import (
	"reflect"
	"testing"
	"time"
)

func TestDisplayOrderingUsesUnicodeFullLowercase(t *testing.T) {
	for _, tc := range []struct{ input, want string }{
		{"İ", "i\u0307"}, {"I", "i"}, {"ΟΣ", "ος"}, {"ΟΣΑ", "οσα"},
		{"Ο\u0301Σ", "ο\u0301ς"}, {"ΟΣ'Α", "οσ'α"}, {"ΟΣ' ", "ος' "}, {"Σ", "σ"},
	} {
		if got := lowercaseTitle(tc.input); got != tc.want {
			t.Errorf("lowercase %q: %q, want %q", tc.input, got, tc.want)
		}
	}
	idx, _ := buildIndex(map[string]Task{"a": NewTask("a", "İ"), "b": NewTask("b", "i")}, "/store")
	if !reflect.DeepEqual(idx.roots, []string{"b", "a"}) {
		t.Fatalf("Unicode ordering: %v", idx.roots)
	}
}

func TestWikiLinksFormsCodeAndUnicode(t *testing.T) {
	cases := []struct {
		name string
		body string
		want []string
	}{
		{"dedupe", "[[one]] [[two]] [[one]]", []string{"one", "two"}},
		{"rich forms", "[[one]] [[one.md|First]] [[sub/one.md|Second]] [[two.md]]", []string{"one", "two"}},
		{"paths", "[[sub\\one.md]] [[./two]]", []string{"one", "two"}},
		{"invalid targets", "[[not a link]] [[UPPER]] [[one.md.md]] [[.md]] [[ok.md]]", []string{"ok"}},
		{"code", "[[keep]]\n```\n[[fenced]]\n```\n~~~\n[[other]]\n~~~\n`[[inline]]`", []string{"keep"}},
		{"unclosed fence", "[[before]]\n```\n[[inside]]\n", []string{"before"}},
		{"unclosed inline", "`[[inside]]\n[[after]]", []string{"after"}},
		{"leftover brackets", "[[one|alias]] b]] [[two]]", []string{"one", "two"}},
		{"multibyte", "muñecos ñ [[one|título ñ]]", []string{"one"}},
		{"unicode fence indent", "\u00a0```\n[[inside]]\n\u00a0```\n[[outside]]", []string{"outside"}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := extractLinks(tc.body); !reflect.DeepEqual(got, tc.want) {
				t.Fatalf("got %v, want %v", got, tc.want)
			}
		})
	}
	body := "niño [[one.md|título ñ]] tail\r\n"
	links := parseWikilinks(body)
	if len(links) != 1 || body[links[0].aliasStart:links[0].aliasEnd] != "título ñ" {
		t.Fatalf("UTF-8 alias span: %#v", links)
	}
}

func TestMirrorAliasRewritesAreSelectiveAndBytePreserving(t *testing.T) {
	cases := []struct {
		body string
		want string
	}{
		{"one [[id.md|Old]] two [[id.md|Context]] and [[id.md]]", "one [[id.md|New]] two [[id.md|Context]] and [[id.md]]"},
		{"`[[id|Old]]`\n```\n[[id|Old]]\n```\n[[id|Old]]", "`[[id|Old]]`\n```\n[[id|Old]]\n```\n[[id|New]]"},
		{"[[id|Old]] b]] [[id|]] [[id]]", "[[id|New]] b]] [[id|]] [[id]]"},
		{"> quote [[id|Old]] *em* <b>html</b>\r\n", "> quote [[id|New]] *em* <b>html</b>\r\n"},
		{"[[id|  Old  ]]", "[[id|New]]"},
	}
	for _, tc := range cases {
		got, changed := rewriteMirrors(tc.body, map[string]titleChange{"id": {"Old", "New"}})
		if !changed || got != tc.want {
			t.Errorf("rewrite %q: %q, %t", tc.body, got, changed)
		}
	}
	got, changed := rewriteMirrors("[[one|A]] [[two|B]]", map[string]titleChange{"one": {"A", "B"}, "two": {"B", "C"}})
	if !changed || got != "[[one|B]] [[two|C]]" {
		t.Fatalf("title diffs must not chase one another: %q", got)
	}
	for _, body := range []string{"[[id|New]]", "[[id|Context]]", "plain body", "[[id]]"} {
		if got, changed := rewriteMirrors(body, map[string]titleChange{"id": {"Old", "New"}}); changed || got != body {
			t.Errorf("unexpected rewrite: %q -> %q", body, got)
		}
	}
	if _, changed := rewriteMirrors("[[id|Same]]", map[string]titleChange{"id": {"Same", "Same"}}); changed {
		t.Fatal("same title must be a no-op")
	}
}

func TestLinkTargetAndTagBoundaries(t *testing.T) {
	for _, target := range []string{"abc", "  abc  ", "abc.md", "sub/abc.md", "sub\\abc", "./abc.md"} {
		if got, ok := linkTarget(target); !ok || got != "abc" {
			t.Errorf("target %q -> %q, %t", target, got, ok)
		}
	}
	for _, target := range []string{"", ".md", "abc.md.md", "UPPER.md", "not a link", "a b.md"} {
		if _, ok := linkTarget(target); ok {
			t.Errorf("invalid target accepted: %q", target)
		}
	}
	query := "work"
	for _, tc := range []struct {
		tag  string
		want bool
	}{{"work", true}, {"work/admin", true}, {"work/admin/x", true}, {"workshop", false}, {"home", false}} {
		task := NewTask("a", "A")
		task.Tags = []string{tc.tag}
		if got := matches(task, Filter{Tag: &query}, time.Time{}); got != tc.want {
			t.Errorf("tag %s: %t", tc.tag, got)
		}
	}
}
