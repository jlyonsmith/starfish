# list all available recipes
list:
  just --list

# Create the PostgreSQLdatabase
create-db:
  ./scripts/create-db.fish

# Run all generation recipes
gen-all:
    just gen-msg
    just odin_ui/gen-all

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

# Generate Geno message sources
gen-msg:
    geno --output-path sf_admin_msg/src/msgs.rs --format rust-serde msgs/admin.geno

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

  if test -e 'justfile' -o -e 'Justfile'
    just cov-json
  else
    cargo test
  end

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

  info "Finished release of '"$name"' on branch '"$branch"'. You can publish the crate."
  exit 0

# Delete the last tag (used for rolling back a release)
del-last-tag:
  #!/usr/bin/env fish
  set tagName (cat "scratch/version.tag.txt")

  git tag -d $tagName
  git push origin --delete $tagName
