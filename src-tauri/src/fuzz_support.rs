//! 单元测试共享的确定性「畸形输入」生成器。
//!
//! 与 `tests/stress_support`（集成层、独立进程、只走命令行与磁盘事实）不同，这一份只在
//! `cfg(test)` 下编译，供各**解析器**的单元测试做畸形输入模糊：容器（`chunk_file`）、
//! DDS 头（`pin_board::dds`）、C2PA 清单（`authenticity::c2pa`）、设置文件
//! （`app::settings`）都是「读取不受信任的字节」的代码，最有效的验证方式是喂进大量
//! **确定性**生成的畸形字节，断言它们**要么干净拒绝、要么正确接受，绝不 panic /
//! 越界 / 无界分配 / 无限循环**。
//!
//! 用自写的 xorshift 而不是引入 proptest：零新依赖、种子固定可复现（失败时把种子带进
//! 断言消息即可原样重放），与本项目既有的属性测试风格（`backup::test_support`）一致。
//! 该模块是 [`crate::backup::test_support::Rng`] 的唯一实现，后者只是转出，避免两份伪随机源。
#![allow(dead_code)]

/// 确定性 xorshift 伪随机源。
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        // 0 是 xorshift 的固定点，用 1 兜底。
        Self(seed | 1)
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }

    /// 返回 `[0, bound)` 内的值；`bound` 为 0 时返回 0。
    pub(crate) fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            (self.next_u64() % bound as u64) as usize
        }
    }

    /// 非零随机字节，用于保证「改动后内容一定与改动前不同」。
    pub(crate) fn nonzero_byte(&mut self) -> u8 {
        (self.next_u64() as u8) | 1
    }
}

/// 由种子直接生成一列确定性伪随机字节。
pub(crate) fn random_bytes(seed: u64, len: usize) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    (0..len).map(|_| rng.next_u64() as u8).collect()
}

/// 对一列字节施加 `ops` 次随机扰动，返回**新**的一列（输入不变）。
///
/// 扰动的种类刻意覆盖真实损坏的各种形态，而不是只翻一个 bit：
/// 位翻转、整字节替换、插入随机片段（改变长度）、删除一段（截断）、两段互换、
/// 把一段清零、整体截掉尾部。取值都从 [`Rng`] 推导，因此同一 `(seed, bytes, ops)`
/// 永远得到同一结果。
pub(crate) fn mutated(seed: u64, bytes: &[u8], ops: usize) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    let mut value = bytes.to_vec();
    for _ in 0..ops {
        if value.is_empty() {
            // 空输入也要能继续扰动：插入一段随机字节。
            let len = 1 + rng.below(16);
            value = random_bytes(rng.next_u64(), len);
            continue;
        }
        match rng.below(8) {
            // 翻转一个 bit。
            0 => {
                let index = rng.below(value.len());
                value[index] ^= 1 << rng.below(8);
            }
            // 整字节替换（保证与原来不同）。
            1 => {
                let index = rng.below(value.len());
                let original = value[index];
                loop {
                    let replacement = rng.next_u64() as u8;
                    if replacement != original {
                        value[index] = replacement;
                        break;
                    }
                }
            }
            // 在随机位置插入一段随机字节（长度变化）。
            2 => {
                let at = rng.below(value.len() + 1);
                let len = 1 + rng.below(24);
                let insertion = random_bytes(rng.next_u64(), len);
                value.splice(at..at, insertion);
            }
            // 删除一段（含把尾部截掉的情形）。
            3 => {
                let at = rng.below(value.len());
                let len = 1 + rng.below(value.len() - at);
                value.drain(at..at + len);
            }
            // 把一段清零。
            4 => {
                let at = rng.below(value.len());
                let len = 1 + rng.below(value.len() - at);
                value[at..at + len].fill(0);
            }
            // 两段互换（长度不变、位置错乱）。
            5 => {
                let left = rng.below(value.len());
                let right = rng.below(value.len());
                value.swap(left, right);
            }
            // 把一整段填充为同一个字节（模拟被整块覆写）。
            6 => {
                let at = rng.below(value.len());
                let len = 1 + rng.below(value.len() - at);
                let fill = rng.next_u64() as u8;
                value[at..at + len].fill(fill);
            }
            // 追加一段随机字节（文件被续写 / 拼接）。
            _ => {
                let len = 1 + rng.below(24);
                value.extend_from_slice(&random_bytes(rng.next_u64(), len));
            }
        }
    }
    value
}

/// 只扰动文件头 `header` 字节之内的范围，用于「声明的元数据被改坏」这一类损坏
/// （结构体字段、长度、魔数、计数）——它比整文件随机扰动更容易命中解析器的
/// 边界分支，因此单独提供。
pub(crate) fn mutated_header(seed: u64, bytes: &[u8], header: usize, ops: usize) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    let mut value = bytes.to_vec();
    let span = header.min(value.len());
    for _ in 0..ops {
        if span == 0 {
            break;
        }
        match rng.below(3) {
            0 => {
                let index = rng.below(span);
                value[index] ^= 1 << rng.below(8);
            }
            1 => value[rng.below(span)] = rng.next_u64() as u8,
            _ => {
                let index = rng.below(span);
                value[index] = value[index].wrapping_add(1);
            }
        }
    }
    value
}
