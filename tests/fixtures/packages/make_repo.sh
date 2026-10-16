#!/bin/sh
# Builds a bare git repository of the Tally fixture package with release
# tags (1.0.0, 1.1.0, 1.2.0, 2.0.0) and a `develop` branch, so package
# tests work offline. Usage: make_repo.sh <dir>; prints the repository path.
set -e
out="$1"
here="$(cd "$(dirname "$0")" && pwd)"
work="$out/Tally-work"
repo="$out/Tally.git"
rm -rf "$work" "$repo"
mkdir -p "$work"
cp -R "$here/Tally/." "$work/"
cd "$work"
git init -q -b main
git config user.name "zcode tests"
git config user.email "tests@zcode.invalid"
git config commit.gpgsign false
git config tag.gpgsign false
release() {
    sed -i '' "s/public static let version = \".*\"/public static let version = \"$1\"/" Sources/Tally/Tally.swift
    git add -A
    git commit -q -m "Tally $1"
    git tag "$1"
}
release 1.0.0
release 1.1.0
release 1.2.0
release 2.0.0
git checkout -q -b develop
sed -i '' 's/public static let version = ".*"/public static let version = "develop"/' Sources/Tally/Tally.swift
git commit -q -am "Work in progress"
git checkout -q main
git clone -q --bare "$work" "$repo"
rm -rf "$work"
echo "$repo"
