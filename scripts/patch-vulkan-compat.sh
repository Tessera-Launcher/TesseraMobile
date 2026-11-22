#!/bin/sh
# Make Minecraft 26.3's Vulkan backend start on Android GPUs that expose the
# KHR spelling of vertex attribute divisor.
#
# This is the compatibility half of the Vulkan patches:
#   1. VulkanFeatureSets requires VK_EXT_vertex_attribute_divisor, but some
#      drivers (Mali-G710 on the Pixel 7, for example) only expose
#      VK_KHR_vertex_attribute_divisor.  The two strings have the same length,
#      so replace the constant in the class file.
#   2. The OIT transmittance shader indexes its fragment-output array with a
#      loop variable.  RenderPearl compiles GLSL with shaderc/glslang before
#      either backend, and glslang rejects that indexing.  Spell the two
#      attachment writes out.  This is not an OpenGL-only workaround.
#
# Usage: scripts/patch-vulkan-compat.sh <client.jar>
# The jar is updated in place.  A backup is made once as <client.jar>.before-vulkan.
set -eu
JAR=${1:?usage: $0 <client.jar>}
if [ ! -f "$JAR" ]; then
    echo "not a file: $JAR" >&2
    exit 1
fi
JAR=$(cd "$(dirname "$JAR")" && pwd -P)/$(basename "$JAR")
for cmd in unzip zip python3; do
    command -v "$cmd" >/dev/null 2>&1 || {
        echo "missing command: $cmd" >&2
        exit 1
    }
done
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
backup="$JAR.before-vulkan"
[ -e "$backup" ] || cp -p "$JAR" "$backup"

# 1. VulkanFeatureSets: EXT -> KHR (same byte length).
class=com/mojang/renderpearl/backend/vulkan/VulkanFeatureSets.class
mkdir -p "$tmp/$(dirname "$class")"
unzip -p "$JAR" "$class" >"$tmp/$class"
state=$(python3 - "$tmp/$class" <<'PY'
import pathlib, sys
path = pathlib.Path(sys.argv[1])
data = path.read_bytes()
old = b"VK_EXT_vertex_attribute_divisor"
new = b"VK_KHR_vertex_attribute_divisor"
if old in data:
    path.write_bytes(data.replace(old, new))
    print("changed")
elif new in data:
    print("already")
else:
    raise SystemExit("VulkanFeatureSets: neither EXT nor KHR extension string found")
PY
)
if [ "$state" = changed ]; then
    (cd "$tmp" && zip -q -u "$JAR" "$class")
fi
echo "VulkanFeatureSets: $state"

# 2. OIT transmittance shader: unroll the output-array writes.
asset=assets/minecraft/shaders/include/oit_add_transmittance.glsl
mkdir -p "$tmp/$(dirname "$asset")"
unzip -p "$JAR" "$asset" >"$tmp/$asset"
state=$(python3 - "$tmp/$asset" <<'PY'
import sys
from pathlib import Path
path = Path(sys.argv[1])
text = path.read_text()
old = '''    for (int attachmentIndex = 0; attachmentIndex < OIT_COEFF_ATTACHMENT_COUNT; attachmentIndex++) {
        for (int i = 0; i < 4; i++) {
            coeff[attachmentIndex][i] = coefficients[attachmentIndex * 4 + i];
        }
    }'''
new = '''    // Android GLSL compilers reject dynamic indexing into fragment outputs.
    // OIT_COEFF_ATTACHMENT_COUNT is 2 for 26.3.
    coeff[0] = vec4(coefficients[0], coefficients[1], coefficients[2], coefficients[3]);
    coeff[1] = vec4(coefficients[4], coefficients[5], coefficients[6], coefficients[7]);'''
if old in text:
    path.write_text(text.replace(old, new))
    print("changed")
elif new in text:
    print("already")
else:
    raise SystemExit("OIT shader: known pattern not found")
PY
)
if [ "$state" = changed ]; then
    (cd "$tmp" && zip -q -u "$JAR" "$asset")
fi
echo "OIT shader: $state"
echo "patched $JAR (backup: $backup)"
