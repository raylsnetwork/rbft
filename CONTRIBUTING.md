# Contributing to RBFT

Thank you for your interest in contributing to RBFT! This document provides guidelines and best practices for contributing to the project.

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Getting Started](#getting-started)
- [Development Workflow](#development-workflow)
- [Code Style](#code-style)
- [Testing](#testing)
- [Commit Guidelines](#commit-guidelines)
- [Pull Request Process](#pull-request-process)
- [Issue Reporting](#issue-reporting)

## Code of Conduct

We are committed to providing a welcoming and inclusive environment. By participating in this project, you agree to:

- Be respectful and considerate of others
- Accept constructive criticism gracefully
- Focus on what is best for the community
- Show empathy towards other community members

## Getting Started

1. **Fork the Repository**
   ```bash
   # Fork on GitHub, then clone your fork
   git clone https://github.com/YOUR_USERNAME/rbft.git
   cd rbft
   ```

2. **Set Up Your Development Environment**
   ```bash
   # Install Rust (if not already installed)
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   
   # Install nightly toolchain for formatting
   rustup toolchain install nightly
   
   # Install pre-commit hooks
   pip install pre-commit
   pre-commit install
   ```

3. **Create a Feature Branch**
   ```bash
   git checkout -b feat/your-feature-name
   # or
   git checkout -b fix/your-bug-fix
   ```

## Development Workflow

### Branch Naming Convention

- `feat/` - New features
- `fix/` - Bug fixes
- `docs/` - Documentation changes
- `refactor/` - Code refactoring
- `test/` - Test additions or modifications
- `chore/` - Maintenance tasks

Examples:
- `feat/add-validator-rotation`
- `fix/consensus-timeout-issue`
- `docs/update-readme`

### Before Starting Work

1. Check existing issues and PRs to avoid duplicate work
2. For major changes, open an issue first to discuss the approach
3. Keep your fork's main branch up to date:
   ```bash
   git remote add upstream https://github.com/raylsnetwork/rbft.git
   git fetch upstream
   git rebase upstream/main
   ```

## Code Style

### Rust Formatting

- **Line Length**: Maximum 100 characters
- **Formatter**: Use `cargo +nightly fmt` (required for unstable rustfmt features)
- **Configuration**: Settings in `rustfmt.toml`

Run before committing:
```bash
cargo +nightly fmt
```

### Code Quality Checks

Run all checks locally before pushing:

```bash
# Format code
cargo +nightly fmt

# Check line lengths
python3 scripts/check_line_length.py

# Run linter
cargo clippy -- -D warnings

# Run tests
cargo test
```

### Best Practices

- **Documentation**: Add doc comments for public APIs
  ```rust
  /// Brief description of the function
  ///
  /// # Arguments
  ///
  /// * `param` - Description of the parameter
  ///
  /// # Returns
  ///
  /// Description of the return value
  ///
  /// # Examples
  ///
  /// ```
  /// let result = function(arg);
  /// ```
  pub fn function(param: Type) -> ReturnType {
      // implementation
  }
  ```

- **Error Handling**: Use `Result` types and provide meaningful error messages
- **Comments**: Write clear comments for complex logic
- **Naming**: Use descriptive names following Rust conventions
  - `snake_case` for functions and variables
  - `CamelCase` for types and traits
  - `SCREAMING_SNAKE_CASE` for constants

## Testing

### Running Tests

```bash
# Run all tests
cargo test

# Run specific test
cargo test test_name

# Run tests for a specific package
cargo test -p rbft-utils

# Run with output
cargo test -- --nocapture
```

### Test Requirements

- **Unit Tests**: Add tests for new functionality
- **Integration Tests**: Test component interactions
- **Documentation Tests**: Ensure example code works

### Test Coverage

We aim for high test coverage. When adding new features:

1. Write tests for happy paths
2. Write tests for error conditions
3. Write tests for edge cases
4. Add integration tests where appropriate

Example test structure:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_functionality() {
        // Test implementation
    }

    #[test]
    #[should_panic(expected = "error message")]
    fn test_error_condition() {
        // Test that should panic
    }
}
```

## Commit Guidelines

### Commit Message Format

```
<type>: <subject>

<body>

<footer>
```

### Types

- `feat`: New feature
- `fix`: Bug fix
- `docs`: Documentation changes
- `style`: Code style changes (formatting, etc.)
- `refactor`: Code refactoring
- `test`: Test additions or modifications
- `chore`: Maintenance tasks

### Examples

```
feat: add validator rotation mechanism

Implement automatic validator rotation based on epoch boundaries.
Includes tests for rotation logic and edge cases.

Closes #123
```

```
fix: resolve consensus timeout in high-latency networks

Increase timeout values and add exponential backoff for retry logic.
This fixes issues observed in networks with >500ms latency.

Fixes #456
```

### Commit Best Practices

- Keep commits atomic (one logical change per commit)
- Write clear, descriptive commit messages
- Reference related issues in commit messages
- Avoid committing generated files or build artifacts
- Don't commit commented-out code

## Pull Request Process

### Before Submitting

1. **Ensure all tests pass**
   ```bash
   cargo test
   ```

2. **Run code quality checks**
   ```bash
   cargo +nightly fmt
   cargo clippy -- -D warnings
   python3 scripts/check_line_length.py
   ```

3. **Update documentation** if you've changed APIs or added features

4. **Rebase on latest main**
   ```bash
   git fetch upstream
   git rebase upstream/main
   ```

### Submitting a Pull Request

1. **Push your branch**
   ```bash
   git push origin feat/your-feature-name
   ```

2. **Open a Pull Request** on GitHub

3. **Fill out the PR template** with:
   - Clear description of changes
   - Related issue numbers
   - Testing performed
   - Any breaking changes
   - Screenshots (if UI changes)

### PR Title Format

```
<type>: <description>
```

Examples:
- `feat: Add ERC20 contract testing framework`
- `fix: Resolve memory leak in consensus module`
- `docs: Update installation instructions`

### Review Process

- PRs require at least one approval from a maintainer
- Address review feedback promptly
- Keep PRs focused and reasonably sized
- Be responsive to questions and suggestions
- CI checks must pass before merge

### After Your PR is Merged

1. Delete your feature branch (GitHub will prompt)
2. Update your local repository:
   ```bash
   git checkout main
   git pull upstream main
   git push origin main
   ```

## Issue Reporting

### Before Creating an Issue

1. Search existing issues to avoid duplicates
2. Check if it's already fixed in the latest version
3. Gather relevant information (logs, environment details)

### Issue Template

When creating an issue, include:

**Bug Reports:**
- Clear description of the problem
- Steps to reproduce
- Expected behavior vs actual behavior
- Environment details (OS, Rust version, etc.)
- Relevant logs or error messages
- Minimal reproduction code if applicable

**Feature Requests:**
- Clear description of the proposed feature
- Use case and motivation
- Potential implementation approach
- Any alternative solutions considered

**Questions:**
- What you're trying to accomplish
- What you've already tried
- Relevant code snippets or configurations

### Labels

We use labels to categorize issues:
- `bug` - Something isn't working
- `enhancement` - New feature or request
- `documentation` - Documentation improvements
- `good first issue` - Good for newcomers
- `help wanted` - Extra attention needed
- `question` - Further information requested

## Development Tips

### Useful Commands

```bash
# Build in release mode
cargo build --release

# Run a specific binary
cargo run --bin rbft-node

# Check without building
cargo check

# Generate documentation
cargo doc --open

# Clean build artifacts
cargo clean

# Run benchmarks
cargo bench
```

### Debugging

```bash
# Run with debug output
RUST_LOG=debug cargo run --bin rbft-node

# Run with trace-level logging
RUST_LOG=trace cargo run --bin rbft-node

# Debug specific module
RUST_LOG=rbft::consensus=debug cargo run --bin rbft-node
```

### Performance Profiling

```bash
# Build with profiling enabled
cargo build --release --features profiling

# Run with CPU profiling
cargo flamegraph --bin rbft-node
```

## Getting Help

- **Documentation**: Check the [docs](doc/) directory
- **Issues**: Search or create an issue on GitHub
- **Discussions**: Use GitHub Discussions for questions and ideas

## Recognition

Contributors will be recognized in:
- Release notes for significant contributions
- The project's contributor list
- Individual PR acknowledgments

Thank you for contributing to RBFT! 🚀
