alias skill := install_skill

# List the available recipes.
default:
    @just --list

# Check formatting without modifying files.
fmt:
    test -z "$(gofmt -l .)"
    stylua --check lua plugin tests/nvim

# Apply gofmt and StyLua to the sources.
fmt-fix:
    gofmt -w .
    stylua lua plugin tests/nvim

# Run go vet.
vet:
    go vet ./...

# Run the Go test suite.
test: build
    go test ./...

# Run the headless Neovim tests against bin/tt.
test-nvim: build
    nvim --headless -u NONE -l tests/nvim/run.lua

# Run every quality gate: fmt, vet, test, test-nvim.
check: fmt vet test test-nvim

# Build the binary the plugin uses.
build:
    go build -o bin/tt ./cmd/tt

# Install the CLI into ~/.local/bin.
install:
    GOBIN="$HOME/.local/bin" go install ./cmd/tt

install_skill:
    rm -rf ~/.agents/skills/tt | true
    mkdir -p ~/.agents/skills/tt
    ln -sf ~/personal/tt/SKILL.md ~/.agents/skills/tt/SKILL.md
