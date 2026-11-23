//! Android client-jar patches, applied by the launcher itself.
//!
//! A 26.x client assumes a desktop GPU stack: it probes the EXT spelling of
//! vertex attribute divisor, compiles an OIT shader with dynamic fragment-output
//! indexing, and keeps an OpenGL fallback. Tessera Mobile is Vulkan-only, and
//! its drivers expose the KHR spelling. The same edits used to live in host
//! scripts; this module applies them during install, so a fresh install works
//! without a workstation.
//!
//! The patches are bytecode/text edits, not a Java compiler: the device has no
//! javac. `PreferredGraphicsApi.getBackendsToTry()` is replaced with a small
//! straight-line method that constructs only `VulkanBackend`, and
//! `NativeLibrariesBootstrap.loadOpenGL()` becomes a bare `return`.

use std::collections::HashMap;
use std::path::Path;

use crate::jar;

const FEATURE_SETS: &str = "com/mojang/renderpearl/backend/vulkan/VulkanFeatureSets.class";
const NATIVE_BOOTSTRAP: &str = "com/mojang/blaze3d/platform/NativeLibrariesBootstrap.class";
const PREFERRED_API: &str = "net/minecraft/client/PreferredGraphicsApi.class";
const OIT_SHADER: &str = "assets/minecraft/shaders/include/oit_add_transmittance.glsl";

const EXT: &[u8] = b"VK_EXT_vertex_attribute_divisor";
const KHR: &[u8] = b"VK_KHR_vertex_attribute_divisor";

const OIT_OLD: &str = "    for (int attachmentIndex = 0; attachmentIndex < OIT_COEFF_ATTACHMENT_COUNT; attachmentIndex++) {\n        for (int i = 0; i < 4; i++) {\n            coeff[attachmentIndex][i] = coefficients[attachmentIndex * 4 + i];\n        }\n    }";
const OIT_NEW: &str = "    // Android GLSL compilers reject dynamic indexing into fragment outputs.\n    // OIT_COEFF_ATTACHMENT_COUNT is 2 for 26.3.\n    coeff[0] = vec4(coefficients[0], coefficients[1], coefficients[2], coefficients[3]);\n    coeff[1] = vec4(coefficients[4], coefficients[5], coefficients[6], coefficients[7]);";

/// What happened to one client jar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// No RenderPearl/Vulkan client classes: an older or non-26.x client.
    NotApplicable,
    /// All four edits were already present.
    AlreadyPatched,
    /// At least one entry was rewritten.
    Patched,
}

/// Apply the Android Vulkan patches to `client.jar` in place.
pub fn apply(path: &Path) -> Result<Outcome, String> {
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(
        std::fs::File::open(path)
            .map_err(|error| format!("无法读取 {}: {error}", path.display()))?,
    ))
    .map_err(|error| format!("无法解析 {}: {error}", path.display()))?;

    let read = |archive: &mut zip::ZipArchive<std::io::BufReader<std::fs::File>>,
                name: &str|
     -> Result<Option<Vec<u8>>, String> {
        match archive.by_name(name) {
            Ok(mut entry) => {
                let mut body = Vec::new();
                std::io::Read::read_to_end(&mut entry, &mut body)
                    .map_err(|error| format!("无法读取 jar 条目 {name}: {error}"))?;
                Ok(Some(body))
            }
            Err(zip::result::ZipError::FileNotFound) => Ok(None),
            Err(error) => Err(format!("无法读取 jar 条目 {name}: {error}")),
        }
    };

    let feature = read(&mut archive, FEATURE_SETS)?;
    let bootstrap = read(&mut archive, NATIVE_BOOTSTRAP)?;
    let preferred = read(&mut archive, PREFERRED_API)?;
    let shader = read(&mut archive, OIT_SHADER)?;
    drop(archive);

    // These two classes are the marker for a client that has RenderPearl.
    // Older versions have no Vulkan backend at all, so there is nothing to do.
    if feature.is_none() && preferred.is_none() {
        return Ok(Outcome::NotApplicable);
    }
    let feature = feature.ok_or_else(|| format!("客户端缺少 {FEATURE_SETS}"))?;
    let bootstrap = bootstrap.ok_or_else(|| format!("客户端缺少 {NATIVE_BOOTSTRAP}"))?;
    let preferred = preferred.ok_or_else(|| format!("客户端缺少 {PREFERRED_API}"))?;
    let shader = shader.ok_or_else(|| format!("客户端缺少 {OIT_SHADER}"))?;

    let mut replacements: HashMap<String, Vec<u8>> = HashMap::new();

    let mut feature = feature;
    if feature.windows(KHR.len()).any(|window| window == KHR) {
        // Already patched.
    } else if let Some(offset) = feature.windows(EXT.len()).position(|window| window == EXT) {
        feature[offset..offset + EXT.len()].copy_from_slice(KHR);
        replacements.insert(FEATURE_SETS.to_string(), feature);
    } else {
        return Err(format!("{FEATURE_SETS}: 找不到 EXT/KHR 扩展名"));
    }

    let mut bootstrap = bootstrap;
    if let Some(patched) = patch_load_open_gl(&bootstrap)? {
        bootstrap = patched;
        replacements.insert(NATIVE_BOOTSTRAP.to_string(), bootstrap);
    }

    let mut preferred = preferred;
    if let Some(patched) = patch_preferred_api(&preferred)? {
        preferred = patched;
        replacements.insert(PREFERRED_API.to_string(), preferred);
    }

    let mut shader = shader;
    let text =
        String::from_utf8(shader).map_err(|error| format!("{OIT_SHADER} 不是 UTF-8: {error}"))?;
    if text.contains(OIT_NEW) {
        // Already unrolled.
    } else if text.contains(OIT_OLD) {
        shader = text.replace(OIT_OLD, OIT_NEW).into_bytes();
        replacements.insert(OIT_SHADER.to_string(), shader);
    } else {
        return Err(format!("{OIT_SHADER}: 找不到已知的输出数组循环"));
    }

    if replacements.is_empty() {
        return Ok(Outcome::AlreadyPatched);
    }
    if jar::replace_entries(path, &replacements)? {
        Ok(Outcome::Patched)
    } else {
        Ok(Outcome::AlreadyPatched)
    }
}

/// Replace `getBackendsToTry()` with `return new GpuBackend[]{new VulkanBackend()};`.
fn patch_preferred_api(data: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let (_, pool) = read_pool(data)?;
    let vulkan = *pool
        .classes
        .get("com/mojang/renderpearl/backend/vulkan/VulkanBackend")
        .ok_or_else(|| "PreferredGraphicsApi: 找不到 VulkanBackend".to_string())?;
    let gpu = *pool
        .classes
        .get("com/mojang/renderpearl/api/device/GpuBackend")
        .ok_or_else(|| "PreferredGraphicsApi: 找不到 GpuBackend".to_string())?;
    let ctor = *pool
        .methods
        .get(&(vulkan, "<init>".to_string(), "()V".to_string()))
        .ok_or_else(|| "PreferredGraphicsApi: 找不到 VulkanBackend.<init>".to_string())?;

    let mut code = Vec::with_capacity(17);
    code.extend_from_slice(&[0xbb]); // new
    push_u2(&mut code, vulkan);
    code.push(0x59); // dup
    code.extend_from_slice(&[0xb7]); // invokespecial
    push_u2(&mut code, ctor);
    code.push(0x4c); // astore_1
    code.push(0x04); // iconst_1
    code.push(0xbd); // anewarray
    push_u2(&mut code, gpu);
    code.push(0x59); // dup
    code.push(0x03); // iconst_0
    code.push(0x2b); // aload_1
    code.push(0x53); // aastore
    code.push(0xb0); // areturn

    patch_method(
        data,
        &pool,
        "getBackendsToTry",
        "()[Lcom/mojang/renderpearl/api/device/GpuBackend;",
        code,
        4,
        2,
    )
}

/// Replace `loadOpenGL()` with a bare `return`.
fn patch_load_open_gl(data: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let (_, pool) = read_pool(data)?;
    patch_method(data, &pool, "loadOpenGL", "()V", vec![0xb1], 0, 0)
}

fn patch_method(
    data: &[u8],
    pool: &Pool,
    method: &str,
    descriptor: &str,
    code: Vec<u8>,
    max_stack: u16,
    max_locals: u16,
) -> Result<Option<Vec<u8>>, String> {
    if data.len() < 10 {
        return Err("class 文件过短".to_string());
    }
    let mut p = 8;
    let _constant_pool_count = read_u2(data, &mut p)?;
    // The pool was parsed by read_pool; this second pass only needs the offset
    // after it. Re-read the whole pool to keep bounds checks in one place.
    p = 8;
    let count = read_u2(data, &mut p)?;
    let mut index = 1;
    while index < count {
        let tag = read_u1(data, &mut p)?;
        match tag {
            1 => {
                let len = read_u2(data, &mut p)? as usize;
                skip(data, &mut p, len)?;
            }
            3 | 4 | 9 | 10 | 11 | 12 | 17 | 18 => skip(data, &mut p, 4)?,
            5 | 6 => {
                skip(data, &mut p, 8)?;
                index += 1;
            }
            7 | 8 | 16 | 19 | 20 => skip(data, &mut p, 2)?,
            15 => skip(data, &mut p, 3)?,
            _ => return Err(format!("未知 constant pool tag {tag}")),
        }
        index += 1;
    }

    skip(data, &mut p, 6)?; // access_flags, this_class, super_class
    let interfaces = read_u2(data, &mut p)? as usize;
    skip(data, &mut p, interfaces * 2)?;

    p = skip_members(data, p)?; // fields
    let methods = read_u2(data, &mut p)? as usize;
    for _ in 0..methods {
        let _access = read_u2(data, &mut p)?;
        let name_index = read_u2(data, &mut p)?;
        let desc_index = read_u2(data, &mut p)?;
        let attributes = read_u2(data, &mut p)? as usize;
        let this_method = pool.utf8.get(&name_index).map(String::as_str) == Some(method)
            && pool.utf8.get(&desc_index).map(String::as_str) == Some(descriptor);
        for _ in 0..attributes {
            let attr_start = p;
            let attr_name = read_u2(data, &mut p)?;
            let attr_len = read_u4(data, &mut p)? as usize;
            let info_start = p;
            let info_end = info_start
                .checked_add(attr_len)
                .ok_or_else(|| "class 属性长度溢出".to_string())?;
            if info_end > data.len() {
                return Err("class 属性越界".to_string());
            }
            if this_method && attr_name == pool.code_name {
                let new_info = code_info(&code, max_stack, max_locals);
                if data[info_start..info_end] == new_info {
                    return Ok(None);
                }
                let mut out = Vec::with_capacity(data.len() + new_info.len());
                out.extend_from_slice(&data[..attr_start]);
                push_u2(&mut out, attr_name);
                push_u4(&mut out, new_info.len() as u32);
                out.extend_from_slice(&new_info);
                out.extend_from_slice(&data[info_end..]);
                return Ok(Some(out));
            }
            p = info_end;
        }
        if this_method {
            return Err(format!("{method}{descriptor}: 找不到 Code 属性"));
        }
    }
    Err(format!("{method}{descriptor}: 找不到方法"))
}

fn code_info(code: &[u8], max_stack: u16, max_locals: u16) -> Vec<u8> {
    let mut info = Vec::with_capacity(12 + code.len());
    push_u2(&mut info, max_stack);
    push_u2(&mut info, max_locals);
    push_u4(&mut info, code.len() as u32);
    info.extend_from_slice(code);
    push_u2(&mut info, 0); // exception_table_length
    push_u2(&mut info, 0); // attributes_count
    info
}

fn skip_members(data: &[u8], mut p: usize) -> Result<usize, String> {
    let count = read_u2(data, &mut p)? as usize;
    for _ in 0..count {
        skip(data, &mut p, 6)?; // access, name_index, descriptor_index
        let attributes = read_u2(data, &mut p)? as usize;
        for _ in 0..attributes {
            skip(data, &mut p, 2)?; // attribute_name_index
            let len = read_u4(data, &mut p)? as usize;
            skip(data, &mut p, len)?;
        }
    }
    Ok(p)
}

struct Pool {
    utf8: HashMap<u16, String>,
    classes: HashMap<String, u16>,
    methods: HashMap<(u16, String, String), u16>,
    code_name: u16,
}

fn read_pool(data: &[u8]) -> Result<(usize, Pool), String> {
    if data.len() < 10 || data[..4] != [0xca, 0xfe, 0xba, 0xbe] {
        return Err("不是 Java class 文件".to_string());
    }
    let mut p = 8;
    let count = read_u2(data, &mut p)?;
    let mut utf8 = HashMap::new();
    let mut class_entries = Vec::new();
    let mut name_and_types = Vec::new();
    let mut method_entries = Vec::new();
    let mut index = 1;
    while index < count {
        let tag = read_u1(data, &mut p)?;
        match tag {
            1 => {
                let len = read_u2(data, &mut p)? as usize;
                let end = p
                    .checked_add(len)
                    .ok_or_else(|| "constant pool Utf8 长度溢出".to_string())?;
                let text = data
                    .get(p..end)
                    .ok_or_else(|| "constant pool Utf8 越界".to_string())?;
                let text = std::str::from_utf8(text)
                    .map_err(|error| format!("constant pool Utf8 非 UTF-8: {error}"))?;
                utf8.insert(index, text.to_string());
                p = end;
            }
            3 | 4 => skip(data, &mut p, 4)?,
            5 | 6 => {
                skip(data, &mut p, 8)?;
                index += 1;
            }
            7 => class_entries.push((index, read_u2(data, &mut p)?)),
            8 | 16 | 19 | 20 => skip(data, &mut p, 2)?,
            9 | 10 | 11 => {
                let first = read_u2(data, &mut p)?;
                let second = read_u2(data, &mut p)?;
                if tag == 10 {
                    method_entries.push((index, first, second));
                }
            }
            12 => {
                let name = read_u2(data, &mut p)?;
                let desc = read_u2(data, &mut p)?;
                name_and_types.push((index, name, desc));
            }
            15 => skip(data, &mut p, 3)?,
            17 | 18 => skip(data, &mut p, 4)?,
            _ => return Err(format!("未知 constant pool tag {tag}")),
        }
        index += 1;
    }

    let mut names = HashMap::new();
    for (index, text) in &utf8 {
        names.entry(text.clone()).or_insert(*index);
    }
    let code_name = *names
        .get("Code")
        .ok_or_else(|| "class 没有 Code 常量".to_string())?;
    let mut classes = HashMap::new();
    for (index, name_index) in class_entries {
        if let Some(name) = utf8.get(&name_index) {
            classes.insert(name.clone(), index);
        }
    }
    let mut nt_by_index = HashMap::new();
    for (index, name_index, desc_index) in name_and_types {
        if let (Some(name), Some(desc)) = (utf8.get(&name_index), utf8.get(&desc_index)) {
            nt_by_index.insert(index, (name.clone(), desc.clone()));
        }
    }
    let mut methods = HashMap::new();
    for (index, class_index, nt_index) in method_entries {
        if let Some((name, desc)) = nt_by_index.get(&nt_index) {
            methods.insert((class_index, name.clone(), desc.clone()), index);
        }
    }
    Ok((
        p,
        Pool {
            utf8,
            classes,
            methods,
            code_name,
        },
    ))
}

fn read_u1(data: &[u8], p: &mut usize) -> Result<u8, String> {
    let value = *data
        .get(*p)
        .ok_or_else(|| "class 文件意外结束".to_string())?;
    *p += 1;
    Ok(value)
}

fn read_u2(data: &[u8], p: &mut usize) -> Result<u16, String> {
    let bytes = data
        .get(*p..*p + 2)
        .ok_or_else(|| "class 文件意外结束".to_string())?;
    *p += 2;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn read_u4(data: &[u8], p: &mut usize) -> Result<u32, String> {
    let bytes = data
        .get(*p..*p + 4)
        .ok_or_else(|| "class 文件意外结束".to_string())?;
    *p += 4;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn skip(data: &[u8], p: &mut usize, len: usize) -> Result<(), String> {
    let end = p
        .checked_add(len)
        .ok_or_else(|| "class 偏移溢出".to_string())?;
    if end > data.len() {
        return Err("class 文件意外结束".to_string());
    }
    *p = end;
    Ok(())
}

fn push_u2(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn push_u4(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}
