#!/bin/sh
# Compile .pyj files offline with the Pyjinn inside the installed Minescript mod.
# usage: minescript/pyjinn_check.sh FILE.pyj...
set -eu
here=$(cd "$(dirname "$0")" && pwd)
mods="$HOME/Library/Application Support/ModrinthApp/profiles/main/mods"
mod=$(ls "$mods"/minescript-fabric-*.jar | tail -n 1)
cache="${TMPDIR:-/tmp}/pyjinn_check"
mkdir -p "$cache"
# The Pyjinn library is nested inside the mod jar; extract it once per mod version.
jar="$cache/$(basename "$mod" .jar)-pyjinn.jar"
if [ ! -f "$jar" ]; then
  inner=$(unzip -Z1 "$mod" | grep '^META-INF/jars/pyjinn-lib-.*\.jar$')
  unzip -p "$mod" "$inner" > "$jar"
fi
# Gson is provided by Minecraft at runtime, not bundled; any recent version parses the same.
gson=$(find "$HOME/Library/Application Support/ModrinthApp/meta/libraries/com/google/code/gson" -name 'gson-*.jar' | sort -V | tail -n 1)
exec java -cp "$jar:$gson" "$here/PyjinnCheck.java" "$@"
