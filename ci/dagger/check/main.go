package main

import (
	"context"
	"fmt"
	"strings"

	"dagger/muninn-ci/internal/dagger"
)

type MuninnCi struct{}

const (
	rustImage   = "rust:1.99.1-bookworm@sha256:9181b56c2a21ab1d5e73cffe04fc762426d30aa7fac3c91141dd033f49a7fc8b"
	nodeImage   = "node:22.23.3-bookworm-slim@sha256:3ed98443660ff48413b5af19ecefce3b9d50845a901a43420d47cc63a9fddbe9"
	pythonImage = "python:3.12-slim-bookworm@sha256:e697c81897302a4c4333bade000d98038fbb21eb064f8ee1fd42d02ebcd9d337"
)

// Checks runs Muninn's Linux Rust, JavaScript, packaging, and publication tests.
func (m *MuninnCi) Checks(ctx context.Context, source *dagger.Directory) (string, error) {
	var out strings.Builder
	base := dag.Container().From(rustImage).
		WithExec([]string{"sh", "-ec", "apt-get update && apt-get install -y --no-install-recommends build-essential pkg-config libssl-dev libxdo-dev libwebkit2gtk-4.1-dev libsoup-3.0-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev ffmpeg && rm -rf /var/lib/apt/lists/*"}).
		WithDirectory("/src", source).WithWorkdir("/src").
		WithExec([]string{"cargo", "fmt", "--all", "--", "--check"}).
		WithExec([]string{"cargo", "clippy", "--locked", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings"}).
		WithExec([]string{"cargo", "test", "--locked", "--workspace"})
	text, err := base.Stdout(ctx)
	if err != nil {
		return "", fmt.Errorf("Rust workspace tests: %w", err)
	}
	out.WriteString(text)

	node := dag.Container().From(nodeImage).
		WithDirectory("/src", source).WithWorkdir("/src").
		WithExec([]string{"sh", "-ec", "node --test src-tauri/test/*.test.js ui/test/*.test.js build/test_native_player_interactions.js"})
	text, err = node.Stdout(ctx)
	if err != nil {
		return "", fmt.Errorf("JavaScript tests: %w", err)
	}
	out.WriteString(text)

	rustTools := dag.Container().From(rustImage)
	nodeTools := dag.Container().From(nodeImage)
	python := dag.Container().From(pythonImage).
		WithExec([]string{"sh", "-ec", "apt-get update && apt-get install -y --no-install-recommends bash git && rm -rf /var/lib/apt/lists/*"}).
		WithDirectory("/usr/local/cargo", rustTools.Directory("/usr/local/cargo")).
		WithDirectory("/usr/local/rustup", rustTools.Directory("/usr/local/rustup")).
		WithFile("/usr/local/bin/node", nodeTools.File("/usr/local/bin/node")).
		WithEnvVariable("CARGO_HOME", "/usr/local/cargo").
		WithEnvVariable("RUSTUP_HOME", "/usr/local/rustup").
		WithEnvVariable("PATH", "/usr/local/cargo/bin:/usr/local/bin:/usr/bin:/bin").
		WithDirectory("/src", source).WithWorkdir("/src").
		WithExec([]string{"sh", "-ec", "python3 -m unittest discover -s build -p 'test_*.py' -v && python3 build/updater-smoke/test_protocol.py"})
	text, err = python.Stdout(ctx)
	if err != nil {
		return "", fmt.Errorf("Python packaging and publication checks: %w", err)
	}
	out.WriteString(text)
	return out.String(), nil
}
