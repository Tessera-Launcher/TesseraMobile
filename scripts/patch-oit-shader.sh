#!/bin/sh
# Patch Minecraft 26.3's OIT transmittance shader for Android GLSL compilers.
#
# The vanilla shader declares a fragment-output array and writes it through
# loop variables:
#
#   layout(location = 0) out vec4 coeff[OIT_COEFF_ATTACHMENT_COUNT];
#   for (int attachment = 0; attachment < ...; attachment++)
#       for (int i = 0; i < 4; i++)
#           coeff[attachment][i] = ...
#
# Mali's GLSL compiler rejects dynamic indexing of output arrays.  For 26.3
# OIT_COEFF_ATTACHMENT_COUNT is 2; spell both attachments out explicitly.
#
# Usage: scripts/patch-oit-shader.sh <client.jar>
set -e
JAR=${1:?usage: $0 <client.jar>}
ASSET=assets/minecraft/shaders/include/oit_add_transmittance.glsl
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/$(dirname "$ASSET")"
unzip -p "$JAR" "$ASSET" > "$TMP/$ASSET"
python3 - "$TMP/$ASSET" <<'PY'
import sys
p = sys.argv[1]
s = open(p).read()
old = '''    for (int attachmentIndex = 0; attachmentIndex < OIT_COEFF_ATTACHMENT_COUNT; attachmentIndex++) {
        for (int i = 0; i < 4; i++) {
            coeff[attachmentIndex][i] = coefficients[attachmentIndex * 4 + i];
        }
    }'''
new = '''    // Android GLSL compilers reject dynamic indexing into fragment outputs.
    // OIT_COEFF_ATTACHMENT_COUNT is 2 for 26.3.
    coeff[0] = vec4(coefficients[0], coefficients[1], coefficients[2], coefficients[3]);
    coeff[1] = vec4(coefficients[4], coefficients[5], coefficients[6], coefficients[7]);'''
if old not in s:
    raise SystemExit('OIT shader pattern not found; already patched?')
open(p, 'w').write(s.replace(old, new))
PY
(cd "$TMP" && zip -q -u "$JAR" "$ASSET")
echo "patched OIT shader in $JAR"
