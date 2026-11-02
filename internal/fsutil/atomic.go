// Package fsutil contains the atomic writers shared by the registry and Stores.
package fsutil

import (
	"crypto/rand"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
)

// Mode specifies publication and parent-directory behavior for Write.
type Mode uint8

const (
	Replace Mode = iota + 1
	CreateNew
	ReplaceWithParents
	CreateNewWithParents
)

// Write writes and syncs a temporary file beside path. CreateNew modes publish
// with a hard link only if the destination does not already exist.
func Write(path string, contents []byte, mode Mode) (bool, error) {
	if mode < Replace || mode > CreateNewWithParents {
		return false, fmt.Errorf("invalid atomic write mode: %d", mode)
	}
	dir := filepath.Dir(path)
	if mode == ReplaceWithParents || mode == CreateNewWithParents {
		if err := os.MkdirAll(dir, 0o777); err != nil {
			return false, err
		}
	}
	var file *os.File
	var temp string
	for attempt := 0; attempt < 100; attempt++ {
		var suffix [6]byte
		if _, err := io.ReadFull(rand.Reader, suffix[:]); err != nil {
			return false, err
		}
		temp = filepath.Join(dir, fmt.Sprintf(".%s.%d.%s.tmp", filepath.Base(path), os.Getpid(), hex.EncodeToString(suffix[:])))
		var err error
		file, err = os.OpenFile(temp, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o666)
		if errors.Is(err, os.ErrExist) {
			continue
		}
		if err != nil {
			return false, err
		}
		break
	}
	if file == nil {
		return false, errors.New("cannot create a unique temporary file")
	}
	defer os.Remove(temp)
	if _, err := file.Write(contents); err != nil {
		file.Close()
		return false, err
	}
	if err := file.Sync(); err != nil {
		file.Close()
		return false, err
	}
	if err := file.Close(); err != nil {
		return false, err
	}
	if mode == CreateNew || mode == CreateNewWithParents {
		if err := os.Link(temp, path); errors.Is(err, os.ErrExist) {
			return false, nil
		} else if err != nil {
			return false, err
		}
	} else if err := os.Rename(temp, path); err != nil {
		return false, err
	}
	return true, nil
}
