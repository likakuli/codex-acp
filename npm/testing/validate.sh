#!/usr/bin/env bash
set -euo pipefail

# Used in CI, extract here for readability

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color
PACKAGE_SCOPE="@likakuli"
BASE_PACKAGE="${PACKAGE_SCOPE}/codex-acp"

echo "NPM Package Setup Validation"
echo "============================="
echo

check_command() {
  if ! command -v "$1" &> /dev/null; then
    echo -e "${RED}✗ Required command not found: $1${NC}"
    exit 1
  fi
}

check_command node
check_command grep

# 1. Validate wrapper script syntax
echo "1. Validating wrapper script syntax..."
if node -c npm/bin/codex-acp.js 2>/dev/null; then
  echo -e "${GREEN}✓ Wrapper script syntax is valid${NC}"
else
  echo -e "${RED}✗ Wrapper script has syntax errors${NC}"
  exit 1
fi
echo

# 2. Validate package.json files
echo "2. Validating package.json files..."
if node -e "JSON.parse(require('fs').readFileSync('npm/package.json', 'utf8'))" 2>/dev/null; then
  echo -e "${GREEN}✓ Base package.json is valid${NC}"
else
  echo -e "${RED}✗ Base package.json is invalid${NC}"
  exit 1
fi

# 3. Check template has required placeholders
echo "3. Validating template placeholders..."
missing_placeholders=0
for placeholder in PACKAGE_NAME VERSION OS ARCH; do
  if ! grep -q "\${${placeholder}}" npm/template/package.json; then
    echo -e "${RED}✗ Template missing ${placeholder} placeholder${NC}"
    missing_placeholders=1
  fi
done

if [ $missing_placeholders -eq 0 ]; then
  echo -e "${GREEN}✓ Template has all required placeholders${NC}"
else
  exit 1
fi
echo

# 4. Check version consistency
echo "4. Checking version consistency..."
CARGO_VERSION=$(grep -m1 "^version" Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
NPM_VERSION=$(node -e "console.log(require('./npm/package.json').version)")

echo "   Cargo.toml version: $CARGO_VERSION"
echo "   npm package.json version: $NPM_VERSION"

if [ "$CARGO_VERSION" != "$NPM_VERSION" ]; then
  echo -e "${RED}✗ Version mismatch${NC}"
  exit 1
fi
echo -e "${GREEN}✓ Versions are in sync${NC}"
echo

# 4b. Verify fork package ownership
echo "4b. Checking package ownership..."
BASE_PACKAGE_NAME=$(node -e "console.log(require('./npm/package.json').name)")
echo "   Base package name: $BASE_PACKAGE_NAME"
if [ "$BASE_PACKAGE_NAME" != "$BASE_PACKAGE" ]; then
  echo -e "${RED}✗ Base package name must be $BASE_PACKAGE${NC}"
  exit 1
fi

if ! grep -q "@likakuli/\${packageName}" npm/bin/codex-acp.js; then
  echo -e "${RED}✗ Wrapper does not resolve platform packages from ${PACKAGE_SCOPE}${NC}"
  exit 1
fi

if ! grep -q '"name": "@likakuli/${PACKAGE_NAME}"' npm/template/package.json; then
  echo -e "${RED}✗ Platform package template does not use ${PACKAGE_SCOPE}${NC}"
  exit 1
fi
echo -e "${GREEN}✓ Package ownership uses ${PACKAGE_SCOPE}${NC}"
echo

# 5. Verify optional dependencies list
echo "5. Verifying platform packages..."
EXPECTED_PACKAGES=(
  "@likakuli/codex-acp-darwin-arm64"
  "@likakuli/codex-acp-darwin-x64"
  "@likakuli/codex-acp-linux-arm64"
  "@likakuli/codex-acp-linux-x64"
  "@likakuli/codex-acp-win32-arm64"
  "@likakuli/codex-acp-win32-x64"
)

missing_packages=0
for pkg in "${EXPECTED_PACKAGES[@]}"; do
  if ! grep -q "\"$pkg\":" npm/package.json; then
    echo -e "${RED}✗ Missing package: $pkg${NC}"
    missing_packages=1
  fi
done

if [ $missing_packages -eq 0 ]; then
  echo -e "${GREEN}✓ All platform packages listed in optionalDependencies${NC}"
else
  exit 1
fi
echo

# 6. Verify release workflow can publish from this fork
echo "6. Verifying release workflow publishing setup..."
if grep -q '!github.event.repository.fork' .github/workflows/release.yml; then
  echo -e "${RED}✗ Release workflow still skips npm publishing for forks${NC}"
  exit 1
fi

if ! grep -q 'secrets.NPM_TOKEN' .github/workflows/release.yml; then
  echo -e "${RED}✗ Release workflow does not reference secrets.NPM_TOKEN for npm publishing${NC}"
  exit 1
fi

if ! grep -q 'npm publish --access public' .github/workflows/release.yml; then
  echo -e "${RED}✗ Release workflow should publish scoped packages with --access public${NC}"
  exit 1
fi

if ! grep -q 'npm publish --access public --tag latest' .github/workflows/release.yml; then
  echo -e "${RED}✗ Release workflow should publish prerelease npm versions with an explicit dist-tag${NC}"
  exit 1
fi

if ! grep -q 'HAS_MACOS_SIGNING' .github/workflows/release.yml || ! grep -q 'HAS_WINDOWS_SIGNING' .github/workflows/release.yml; then
  echo -e "${RED}✗ Release workflow should make macOS/Windows signing optional${NC}"
  exit 1
fi

if ! grep -q 'tag_name=codex-rs-v$VERSION' .github/workflows/release.yml; then
  echo -e "${RED}✗ Release workflow default tag name should follow codex-rs-v<version>${NC}"
  exit 1
fi

if ! grep -q 'target_commitish: \${{ github.sha }}' .github/workflows/release.yml; then
  echo -e "${RED}✗ Release workflow should create releases from the workflow commit SHA${NC}"
  exit 1
fi

if ! grep -q 'reuse_artifacts_run_id' .github/workflows/release.yml; then
  echo -e "${RED}✗ Release workflow should support reusing artifacts from a previous run${NC}"
  exit 1
fi

if ! grep -q "inputs.reuse_artifacts_run_id == ''" .github/workflows/release.yml; then
  echo -e "${RED}✗ Release workflow should skip Rust builds when reusing artifacts${NC}"
  exit 1
fi

if ! grep -q 'gh run download' .github/workflows/release.yml; then
  echo -e "${RED}✗ Release workflow should download previous run artifacts when reuse_artifacts_run_id is set${NC}"
  exit 1
fi

if ! grep -q "tr -d '\\\\r\\\\n'" .github/workflows/release.yml; then
  echo -e "${RED}✗ Release workflow should sanitize NPM_TOKEN before npm publish${NC}"
  exit 1
fi

echo -e "${GREEN}✓ Release workflow publishing setup is fork-ready${NC}"
