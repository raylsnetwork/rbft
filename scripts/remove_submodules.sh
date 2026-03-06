#!/bin/bash
set -e

echo "Removing git submodules and migrating to Soldeer..."

# Remove submodule entries from .gitmodules and .git/config
echo "→ Deinitializing submodules..."
git submodule deinit -f lib/forge-std || true
git submodule deinit -f lib/openzeppelin-contracts-upgradeable || true
git submodule deinit -f lib/openzeppelin-foundry-upgrades || true

# Remove submodule directories from git
echo "→ Removing submodule directories from git..."
git rm -f lib/forge-std || true
git rm -f lib/openzeppelin-contracts-upgradeable || true
git rm -f lib/openzeppelin-foundry-upgrades || true

# Remove .gitmodules file
echo "→ Removing .gitmodules..."
rm -f .gitmodules

# Remove the lib directory physically (if it still exists)
echo "→ Cleaning up lib directory..."
rm -rf lib/

# Update .gitignore to ignore lib directory
echo "→ Updating .gitignore..."
if ! grep -q "^lib/$" .gitignore 2>/dev/null; then
    echo "" >> .gitignore
    echo "# Foundry/Soldeer dependencies" >> .gitignore
    echo "lib/" >> .gitignore
fi

# Note: target/forge_dependencies/ is already covered by target/ in .gitignore

echo "✓ Submodules removed successfully!"
echo ""
echo "Next steps:"
echo "1. Commit the changes: git add . && git commit -m 'Remove git submodules, migrate to Soldeer'"
echo "2. Dependencies will be automatically installed when you run genesis generation"
echo "3. The 'target/forge_dependencies/' directory will be created and managed by soldeer-core"
