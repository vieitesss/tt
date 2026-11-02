package vault

import (
	"errors"
	"strconv"
	"strings"
	"unicode/utf8"

	"go.yaml.in/yaml/v3"
)

// maskNonBMP works around yaml.v3 treating all non-BMP characters as
// non-printable. A printable, collision-free placeholder lets the emitter
// choose the same scalar style it would for literal Unicode. The mapping is
// constructed solely for serialization, so its nodes can be changed here.
func maskNonBMP(root *yaml.Node) (string, []rune) {
	const first, last = '\uE000', '\uF8FF'
	var used [last - first + 1]bool
	var scalars []*yaml.Node
	var fields []string
	hasNonBMP := false
	var visit func(*yaml.Node)
	visit = func(node *yaml.Node) {
		if node.Kind == yaml.ScalarNode {
			scalars = append(scalars, node)
			for _, r := range node.Value {
				if r > 0xFFFF {
					hasNonBMP = true
				}
			}
		}
		for _, field := range []string{node.Value, node.Tag, node.Anchor, node.HeadComment, node.LineComment, node.FootComment} {
			fields = append(fields, field)
			for _, r := range field {
				if r >= first && r <= last {
					used[r-first] = true
				}
			}
		}
		for _, child := range node.Content {
			visit(child)
		}
	}
	visit(root)
	if !hasNonBMP {
		return "", nil
	}
	marker := ""
	for r := first; r <= last; r++ {
		if !used[r-first] {
			marker = string(r)
			break
		}
	}
	if marker == "" {
		// An input may contain every private-use character. Find a sequence
		// that is absent from every field instead of relying on a reserved rune.
		for n := 0; ; n++ {
			candidate := "\uE000ttUnicode" + strconv.Itoa(n) + "\uE001"
			collision := false
			for _, field := range fields {
				if strings.Contains(field, candidate) {
					collision = true
					break
				}
			}
			if !collision {
				marker = candidate
				break
			}
		}
	}
	var original []rune
	for _, node := range scalars {
		value := node.Value
		var replaced strings.Builder
		start := 0
		for i := 0; i < len(value); {
			r, width := utf8.DecodeRuneInString(value[i:])
			if r > 0xFFFF {
				replaced.WriteString(value[start:i])
				replaced.WriteString(marker)
				original = append(original, r)
				start = i + width
			}
			i += width
		}
		if start != 0 {
			replaced.WriteString(value[start:])
			node.Value = replaced.String()
		}
	}
	return marker, original
}

// restoreNonBMP restores placeholders in emitter traversal order. A mismatch
// fails the write instead of risking altered task content.
func restoreNonBMP(encoded, marker string, original []rune) (string, error) {
	if len(original) == 0 {
		return encoded, nil
	}
	var out strings.Builder
	out.Grow(len(encoded))
	for _, r := range original {
		pos := strings.Index(encoded, marker)
		if pos < 0 {
			return "", errors.New("cannot restore Unicode in YAML output")
		}
		out.WriteString(encoded[:pos])
		out.WriteRune(r)
		encoded = encoded[pos+len(marker):]
	}
	if strings.Contains(encoded, marker) {
		return "", errors.New("cannot restore Unicode in YAML output")
	}
	out.WriteString(encoded)
	return out.String(), nil
}
