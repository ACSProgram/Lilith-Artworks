//! 路径的展示表示、命名空间校验与等价性归一。
//!
//! 这些函数的语义是业务需要（展示、拒绝危险路径、比较两条路径是否同一位置），
//! 平台差异（`\\?\` 前缀、分隔符方向、大小写敏感性）是实现细节，全部在本模块
//! 内部消化：函数名不暴露任何平台词。

use std::path::Path;

/// 展示用路径：剥离 Windows 设备命名空间前缀。
///
/// canonicalize 在 Windows 上返回 `\\?\C:\...` 形式，直接展示给用户既难读也不可
/// 再输入；其它平台的路径不会带该前缀，因此行为上等同原样返回。只做词法处理，
/// 不访问文件系统。
pub(crate) fn display_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    value.strip_prefix(r"\\?\").unwrap_or(&value).to_owned()
}

/// 路径的命名空间分类。只做**词法**判断，不访问文件系统：读取侧校验要能在文件
/// 已被移动或删除、甚至从未存在的情况下照常完成。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathNamespace {
    /// 本地卷或普通挂载点。
    Local,
    /// UNC 网络位置：`\\server\share\...` 与 verbatim 形式 `\\?\UNC\server\share\...`。
    /// NAS 与共享目录是合法工作方式，[`PathNamespace::Network`] 本身不是错误。
    Network,
    /// Windows 设备命名空间：`\\.\`、verbatim 前缀 `\\?\`，以及 NT 对象路径
    /// `\Device\`。读取这类路径可绕过盘符与访问控制，因此一律拒绝。
    Device,
}

/// 分类路径命名空间。分隔符 `\` 与 `/` 视为等价，前缀匹配不区分大小写——
/// Windows 对两者都接受，只按 `/` 或原始大小写判断会被绕过。
pub(crate) fn path_namespace(value: &str) -> PathNamespace {
    let normalized = value.trim().replace('/', "\\");
    let lowered = normalized.to_lowercase();
    if lowered.starts_with(r"\\?\unc\") {
        return PathNamespace::Network;
    }
    if lowered.starts_with(r"\\?\")
        || lowered.starts_with(r"\\.\")
        || lowered.starts_with(r"\device\")
    {
        return PathNamespace::Device;
    }
    if lowered.starts_with(r"\\") {
        return PathNamespace::Network;
    }
    PathNamespace::Local
}

/// 拒绝设备命名空间路径；网络位置照常通过。用于用户在场、可自行判断的手动操作
/// （手动提交、在文件夹中显示）。
pub(crate) fn ensure_not_device_path(value: &str, label: &str) -> Result<(), String> {
    if path_namespace(value) == PathNamespace::Device {
        return Err(format!("{label}不能是设备命名空间路径"));
    }
    Ok(())
}

/// 路径的等价性 key：统一分隔符，Windows 上再统一大小写。
///
/// 大小写折叠是 Windows 文件系统（默认配置下）的实现选择，不是本函数的业务
/// 语义——名字只承诺「两条指向同一位置的路径得到同一个 key」。将来的平台差异
/// （macOS 默认不敏感、Linux 敏感）由本函数内部消化。
///
/// **同步要求**：前端的仓库过滤（`src/shared/useRuntimeSubscription.ts` 的
/// `normalizeRepositoryKey`）是 [`display_path`] + 本函数再加「去掉结尾分隔符」的
/// 组合，两边必须同时改，见 `docs/guides/platform.md` 的例外登记表。
pub(crate) fn equivalence_key(value: &str) -> String {
    if cfg!(windows) {
        value.replace('/', "\\").to_lowercase()
    } else {
        value.replace('\\', "/")
    }
}

/// 分支工作文件路径的等价性 key：输入是已经剥过设备前缀的展示路径
/// （[`display_path`] 的产物），Windows 上统一小写，其它平台原样返回。
///
/// 与 [`equivalence_key`] 的差别：调用方拿到的是 canonicalize 之后的路径，分隔符
/// 已是平台原生的，这里不再归一分隔符，只做大小写折叠。两者都只承诺「同一位置
/// 得到同一 key」，选用哪个取决于输入是否可能混用两种分隔符。
pub(crate) fn source_path_key(value: &str) -> String {
    if cfg!(windows) {
        value.to_lowercase()
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_local_network_and_device_namespaces() {
        assert_eq!(path_namespace(r"C:\work\artwork.psd"), PathNamespace::Local);
        assert_eq!(
            path_namespace(r"\\nas\share\artwork.psd"),
            PathNamespace::Network
        );
        assert_eq!(
            path_namespace(r"//nas/share/artwork.psd"),
            PathNamespace::Network
        );
        assert_eq!(
            path_namespace(r"\\?\UNC\nas\share\artwork.psd"),
            PathNamespace::Network
        );
        // verbatim 与设备前缀一律按设备命名空间处理：它们绕过盘符与访问控制，
        // 而写入侧早已把 `\\?\` 前缀剥掉（见 `display_path`），正常数据不会带它。
        assert_eq!(
            path_namespace(r"\\?\C:\work\artwork.psd"),
            PathNamespace::Device
        );
        assert_eq!(path_namespace(r"\\.\PhysicalDrive0"), PathNamespace::Device);
        assert_eq!(
            path_namespace(r"\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy1\x"),
            PathNamespace::Device
        );
        assert_eq!(
            path_namespace(r"\Device\HarddiskVolume1\x"),
            PathNamespace::Device
        );
    }

    #[test]
    fn manual_paths_reject_only_the_device_namespace() {
        ensure_not_device_path(r"\\nas\share\artwork.psd", "工作文件").unwrap();
        let error = ensure_not_device_path(r"\\.\PhysicalDrive0", "工作文件").unwrap_err();
        assert!(error.contains("设备命名空间"), "{error}");
    }

    /// 两个 key 的分工：`equivalence_key` 连分隔符一起归一，`source_path_key` 只折叠
    /// 大小写（输入已是平台原生分隔符）。这里把两条承诺各自钉在测试上。
    #[test]
    fn equivalence_keys_match_their_documented_scope() {
        let mixed = r"C:\Work\Artwork.psd";
        assert_eq!(
            equivalence_key(mixed),
            equivalence_key("C:/Work/Artwork.psd")
        );
        if cfg!(windows) {
            assert_eq!(equivalence_key(mixed), r"c:\work\artwork.psd");
            assert_eq!(source_path_key(mixed), r"c:\work\artwork.psd");
            // 只折叠大小写：不归一分隔符，因此混用分隔符的写法得到不同 key
            // （调用方保证输入已是 canonicalize 后的平台原生形式）。
            assert_ne!(
                source_path_key(mixed),
                source_path_key("C:/Work/Artwork.psd")
            );
        } else {
            assert_eq!(equivalence_key(mixed), "C:/Work/Artwork.psd");
            assert_eq!(source_path_key(mixed), r"C:\Work\Artwork.psd");
        }
    }
}
