#!/bin/sh
# Apply the strict Vulkan-only client patches.
#
# `--graphicsBackend vulkan` only asks Minecraft to *prefer* Vulkan: the
# original PreferredGraphicsApi still returns { Vulkan, OpenGL } for VULKAN,
# so a failed Vulkan device falls back to OpenGL.  This script removes that
# fallback and also makes the eager OpenGL bootstrap a no-op.
#
# It first runs patch-vulkan-compat.sh (EXT -> KHR + OIT shader unroll).
#
# Usage: scripts/patch-vulkan-only.sh <client.jar>
#
# PreferredGraphicsApi is recompiled against the client jar and its two Mojang
# dependencies.  Use a JDK 28 javac (or set JAVAC):
#   JAVAC=/path/to/jdk-28/bin/javac scripts/patch-vulkan-only.sh <client.jar>
# Extra classpath entries can be supplied with EXTRA_CLASSPATH.
set -eu
JAR=${1:?usage: $0 <client.jar>}
if [ ! -f "$JAR" ]; then
    echo "not a file: $JAR" >&2
    exit 1
fi
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
JAR=$(cd "$(dirname "$JAR")" && pwd -P)/$(basename "$JAR")

"$HERE/patch-vulkan-compat.sh" "$JAR"

JAVAC=${JAVAC:-}
if [ -z "$JAVAC" ]; then
    if [ -n "${JAVA_HOME:-}" ] && [ -x "$JAVA_HOME/bin/javac" ]; then
        JAVAC=$JAVA_HOME/bin/javac
    else
        JAVAC=$(command -v javac || true)
    fi
fi
if [ -z "$JAVAC" ] || [ ! -x "$JAVAC" ]; then
    echo "javac not found; set JAVAC=/path/to/jdk-28/bin/javac" >&2
    exit 1
fi

# Find the two dependencies used by PreferredGraphicsApi's signatures.
GAME_ROOT=$(cd "$(dirname "$JAR")/../.." && pwd -P)
find_jar() {
    find "$GAME_ROOT/libraries/com/mojang/$1" -name '*.jar' 2>/dev/null | sort | tail -n 1
}
DFU=$(find_jar datafixerupper)
BRIG=$(find_jar brigadier)
if [ -z "$DFU" ] || [ -z "$BRIG" ]; then
    echo "could not find datafixerupper/brigadier under $GAME_ROOT/libraries" >&2
    echo "set EXTRA_CLASSPATH=path/to/datafixerupper.jar:path/to/brigadier.jar" >&2
    exit 1
fi
CP=$JAR:$DFU:$BRIG
[ -n "${EXTRA_CLASSPATH:-}" ] && CP=$CP:$EXTRA_CLASSPATH

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/src/net/minecraft/client" "$tmp/classes"
cat >"$tmp/src/net/minecraft/client/PreferredGraphicsApi.java" <<'JAVA'
package net.minecraft.client;

import com.mojang.renderpearl.api.device.GpuBackend;
import com.mojang.renderpearl.backend.vulkan.VulkanBackend;
import com.mojang.serialization.Codec;
import net.minecraft.network.chat.Component;
import net.minecraft.util.StringRepresentable;

public enum PreferredGraphicsApi implements StringRepresentable {
    DEFAULT("default", "options.graphicsApi.default"),
    OPENGL("opengl", "options.graphicsApi.opengl"),
    VULKAN("vulkan", "options.graphicsApi.vulkan");

    public static final Codec<PreferredGraphicsApi> CODEC = StringRepresentable.fromEnum(PreferredGraphicsApi::values);
    private final String serializedName;
    private final Component key;

    PreferredGraphicsApi(final String serializedName, final String key) {
        this.serializedName = serializedName;
        this.key = Component.translatable(key);
    }

    public Component caption() {
        return this.key;
    }

    @Override
    public String getSerializedName() {
        return this.serializedName;
    }

    public GpuBackend[] getBackendsToTry() {
        return new GpuBackend[]{new VulkanBackend()};
    }
}
JAVA
"$JAVAC" --release 25 -cp "$CP" -d "$tmp/classes" \
    "$tmp/src/net/minecraft/client/PreferredGraphicsApi.java"

class=net/minecraft/client/PreferredGraphicsApi.class
(cd "$tmp/classes" && zip -q -u "$JAR" "$class")
echo "PreferredGraphicsApi: Vulkan only"

# NativeLibrariesBootstrap.loadOpenGL() becomes `return`; the method has no
# arguments and no locals, so the class file can be rewritten without ASM.
class=com/mojang/blaze3d/platform/NativeLibrariesBootstrap.class
mkdir -p "$tmp/$(dirname "$class")"
unzip -p "$JAR" "$class" >"$tmp/$class"
state=$(python3 - "$tmp/$class" <<'PY'
import struct, sys

def u1(d, p):
    return d[p], p + 1

def u2(d, p):
    return struct.unpack_from(">H", d, p)[0], p + 2

def u4(d, p):
    return struct.unpack_from(">I", d, p)[0], p + 4

def patch(data):
    if data[:4] != b"\xca\xfe\xba\xbe":
        raise SystemExit("not a class file")
    p = 8
    cp_count, p = u2(data, p)
    utf8 = {}
    i = 1
    while i < cp_count:
        tag, p = u1(data, p)
        if tag == 1:
            n, p = u2(data, p)
            utf8[i] = data[p:p + n].decode("utf-8", "replace")
            p += n
        elif tag in (7, 8, 16, 19, 20):
            p += 2
        elif tag == 15:
            p += 3
        elif tag in (3, 4, 9, 10, 11, 12, 17, 18):
            p += 4
        elif tag in (5, 6):
            p += 8
            i += 1
        else:
            raise SystemExit(f"bad constant tag {tag} at cp {i}")
        i += 1

    p += 6  # access, this, super
    interfaces, p = u2(data, p)
    p += 2 * interfaces

    def skip_members(p):
        count, p = u2(data, p)
        for _ in range(count):
            p += 6  # access, name, descriptor
            attrs, p = u2(data, p)
            for _ in range(attrs):
                p += 2  # attribute_name_index
                n, p = u4(data, p)
                p += n
        return p

    p = skip_members(p)  # fields
    methods, p = u2(data, p)
    for _ in range(methods):
        p += 2  # access
        name_idx, p = u2(data, p)
        desc_idx, p = u2(data, p)
        attrs, p = u2(data, p)
        for _ in range(attrs):
            attr_name_idx, p = u2(data, p)
            attr_len, p = u4(data, p)
            info_start = p
            p += attr_len
            if (utf8.get(name_idx) == "loadOpenGL"
                    and utf8.get(desc_idx) == "()V"
                    and utf8.get(attr_name_idx) == "Code"):
                info = data[info_start:info_start + attr_len]
                q = 0
                max_stack, q = u2(info, q)
                max_locals, q = u2(info, q)
                code_len, q = u4(info, q)
                code = info[q:q + code_len]
                if max_stack == 0 and max_locals == 0 and code == b"\xb1":
                    print("already")
                    return data
                new_info = (struct.pack(">HHI", 0, 0, 1) + b"\xb1"
                            + struct.pack(">HH", 0, 0))
                new_attr = (struct.pack(">HI", attr_name_idx, len(new_info))
                            + new_info)
                print("changed")
                return data[:info_start - 6] + new_attr + data[info_start + attr_len:]
    raise SystemExit("loadOpenGL()V Code attribute not found")

data = open(sys.argv[1], "rb").read()
out = patch(data)
if out is not data:
    open(sys.argv[1], "wb").write(out)
PY
)
if [ "$state" = changed ]; then
    (cd "$tmp" && zip -q -u "$JAR" "$class")
fi
echo "NativeLibrariesBootstrap.loadOpenGL(): $state"
echo "patched $JAR for strict Vulkan-only"
