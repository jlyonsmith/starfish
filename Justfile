# list all available recipes
list:
  just --list

# Build the cross compilation image the two Linux builds run in
build-image:
    docker build -f docker/Dockerfile.build -t starfish-build:latest .

# Cross compiling to Darwin would need the macOS SDK inside the container, and
# the host is already the target, so this one is a plain native build.
#
# Build every binary for macOS on Apple silicon (aarch64-apple-darwin)
build-macos:
    cargo build --release --target aarch64-apple-darwin

# Build every binary for 64-bit ARM Linux (aarch64-unknown-linux-gnu)
build-linux-arm64: (cross-build 'aarch64-unknown-linux-gnu')

# Build every binary for 64-bit Intel Linux (x86_64-unknown-linux-gnu)
build-linux-amd64: (cross-build 'x86_64-unknown-linux-gnu')

# Build every binary for every supported target
build-all: build-macos build-linux-arm64 build-linux-amd64

# Run a release build for TARGET in the cross compilation container.  The
# working tree is bind mounted, so the binaries land in target/TARGET/release
# on the host; the registry lives in a named volume so the downloads survive
# between runs and stay out of the working tree.
[private]
cross-build TARGET: build-image
    docker run --rm \
        -v {{justfile_directory()}}:/src \
        -v starfish-cargo-registry:/usr/local/cargo/registry \
        starfish-build:latest \
        cargo build --release --target {{TARGET}}

# Create the PostgreSQLdatabase
create-db:
  ./scripts/create-db.fish

# Run the tests that need nothing external
test:
    cargo test --workspace

# Run every test, including the ones needing PostgreSQL and Docker
test-all: test-db test-ubuntu test-systemd

# Run the controller end-to-end tests.  Drops and recreates the named database.
test-db DATABASE_URL='postgresql://localhost:5432/starfish_test':
    STARFISH_TEST_DATABASE_URL={{DATABASE_URL}} cargo test -p starfishd --test end_to_end

# Build the Ubuntu image the container tests run against
docker-image:
    docker build -f docker/Dockerfile.test -t starfish-test:latest .

# Run the privileged helper against a real Ubuntu in a container
test-ubuntu: docker-image
    STARFISH_TEST_DOCKER=1 cargo test -p starfish_sync --test ubuntu

# Run the agent under systemd on a real Ubuntu VM
test-systemd: docker-image
    ./scripts/test-systemd.sh

# Release a new version
release OPERATION='incrPatch':
  #!/usr/bin/env fish
  function info
    set_color green; echo "👉 "$argv; set_color normal
  end
  function warning
    set_color yellow; echo "🐓 "$argv; set_color normal
  end
  function error
    set_color red; echo "💥 "$argv; set_color normal
  end

  if test ! -e "Cargo.toml"
    error "Cargo.toml file not found"
    exit 1
  end

  info "Checking for uncommitted changes"

  if not git diff-index --quiet HEAD -- > /dev/null 2> /dev/null
    error "There are uncomitted changes - commit or stash them and try again"
    exit 1
  end

  set branch (string trim (git rev-parse --abbrev-ref HEAD 2> /dev/null))
  set name (basename (pwd))

  info "Starting release of '"$name"' on branch '"$branch"'"

  info "Checking out '"$branch"'"
  git checkout $branch

  info "Pulling latest"
  git pull

  mkdir scratch 2> /dev/null

  if not stampver -u {{OPERATION}}
    error "Unable to generate version information"
    exit 1
  end

  set tagName (cat "scratch/version.tag.txt")
  set tagDescription (cat "scratch/version.desc.txt")

  git rev-parse $tagName > /dev/null 2> /dev/null
  if test $status -ne 0; set isNewTag 1; end

  if set -q isNewTag
    info "'"$tagName"' is a new tag"
  else
    warning "Tag '"$tagName"' already exists and will not be moved"
  end

  just test-all

  if test $status -ne 0
    # Rollback
    git checkout $branch .
    error "Tests failed '"$name"' on branch '"$branch"'"
    exit 1
  end

  info "Staging version changes"
  git add :/

  info "Committing version changes"
  git commit -m $tagDescription

  if set -q isNewTag
    info "Tagging"
    git tag -a $tagName -m $tagDescription
  end

  info "Pushing to 'origin'"
  git push --follow-tags

  info "Building the Linux binaries"
  just build-linux-arm64 build-linux-amd64

  info "Finished release of '"$name"' "$tagName". Run 'just publish' to draft the GitHub release."
  exit 0

# Package the release archives into scratch/publish, without uploading anything
bundle TAG='':
  #!/usr/bin/env bash
  # One archive per component per platform, each holding the binaries, the
  # deployment files and the install script that goes with them, so an archive
  # unpacks into a directory the installer can run from unchanged.
  set -euo pipefail

  # An inherited CDPATH makes `cd` echo where it went, and can resolve a
  # relative path somewhere else entirely.
  unset CDPATH

  info() { printf '\033[32m👉 \033[0m%s\n' "$*"; }
  error() { printf '\033[31m💥 \033[0m%s\n' "$*" >&2; exit 1; }

  command -v zip > /dev/null || error "zip is not installed"

  # stampver has already written this version into the crates, so the tag it
  # belongs to is not guessed separately.
  version=$(sed -n 's/^version = "\(.*\)"$/\1/p' starfishd/Cargo.toml | head -1)
  tag='{{TAG}}'
  [ -n "$tag" ] || tag="v$version"

  staging=scratch/publish
  rm -rf "$staging"
  mkdir -p "$staging"

  for target in aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu; do
    out="target/$target/release"

    for binary in starfishd starfish-admin starfish-agent starfish-sync; do
      [ -f "$out/$binary" ] \
        || error "$out/$binary is missing; run 'just build-linux-arm64 build-linux-amd64'"
    done

    agent="starfish-agent-$tag-$target"
    mkdir -p "$staging/$agent"
    cp "$out/starfish-agent" "$out/starfish-sync" "$staging/$agent/"
    cp deploy/starfish-agent.service deploy/starfish-sync.sudoers "$staging/$agent/"
    cp scripts/install-agent.sh README.md "$staging/$agent/"

    controller="starfish-controller-$tag-$target"
    mkdir -p "$staging/$controller"
    cp "$out/starfishd" "$out/starfish-admin" "$staging/$controller/"
    cp deploy/starfishd.service deploy/starfishd.tmpfiles deploy/roles.sql "$staging/$controller/"
    cp scripts/install-controller.sh README.md "$staging/$controller/"

    for dir in "$agent" "$controller"; do
      ( cd "$staging" && zip -qr "$dir.zip" "$dir" && rm -rf "$dir" )
      info "$staging/$dir.zip"
    done
  done

# Upload the release archives to a draft GitHub release
publish TAG='': (bundle TAG)
  #!/usr/bin/env bash
  # The release is left as a *draft*: the notes are written by hand on GitHub
  # before it is published.
  set -euo pipefail

  info() { printf '\033[32m👉 \033[0m%s\n' "$*"; }
  error() { printf '\033[31m💥 \033[0m%s\n' "$*" >&2; exit 1; }

  command -v gh > /dev/null || error "the gh CLI is not installed"
  gh auth status > /dev/null 2>&1 || error "gh is not logged in; run 'gh auth login'"

  version=$(sed -n 's/^version = "\(.*\)"$/\1/p' starfishd/Cargo.toml | head -1)
  tag='{{TAG}}'
  [ -n "$tag" ] || tag="v$version"

  git rev-parse -q --verify "refs/tags/$tag" > /dev/null \
    || error "there is no $tag tag; run 'just release' first"

  if gh release view "$tag" > /dev/null 2>&1; then
    info "Updating the existing $tag release"
  else
    info "Creating the draft $tag release"
    gh release create "$tag" --draft --title "$tag" \
      --notes "Release notes to be written before publishing."
  fi

  gh release upload "$tag" scratch/publish/*.zip --clobber

  info "Draft release ready. Add the notes and publish it:"
  gh release view "$tag" --json url --jq .url

# Delete the last tag (used for rolling back a release)
del-last-tag:
  #!/usr/bin/env fish
  set tagName (cat "scratch/version.tag.txt")

  git tag -d $tagName
  git push origin --delete $tagName
