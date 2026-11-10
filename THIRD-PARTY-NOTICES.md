# 第三方许可与声明

本项目自身以 [PolyForm Noncommercial 1.0.0](LICENSE) 授权。本文件汇总随本项目一同分发的第三方组件及其许可，**不改变**这些组件各自的许可条款。

## 1. 内置字体：Noto Sans SC（子集）

| 项目 | 内容 |
|------|------|
| 文件 | `crates/miconlab-gui/assets/fonts/NotoSansSC-Subset.otf` |
| 大小 | 242,212 字节 |
| SHA-256 | `d0f98447952ce8d713040ca0962bd600e8aecbc0f7376b2ae8c79aae7712bf33` |
| 字体名 | Noto Sans SC（Regular） |
| 许可 | SIL Open Font License 1.1（OFL-1.1） |
| 版权 | Copyright 2014-2021 Adobe (http://www.adobe.com/). Noto is a trademark of Google Inc. |
| 来源 | 由 Noto Sans CJK SC 官方发行版经 `pyftsubset` 子集化生成 |
| 许可全文 | 同名目录下的 `OFL.txt`，以及本文件第 5 节 |

说明：

- 该字体以 OFL-1.1 分发，**不因本项目采用 PolyForm Noncommercial 而被重新授权**。
- 本项目对字体做了子集化（仅保留界面所需字形），属于 OFL-1.1 允许的修改；发布时必须保留上述版权声明与本许可文本。
- 字体本身不得单独售卖。
- **当前源码树内只有上述一个字体文件**，不包含任何系统字体或其他第三方字体文件；GUI 运行期也不再读取系统字体。
- 历史版本说明：早期部分构建产物曾误打包非自由许可字体，与上述声明不符。该字体未获随附分发授权，相关产物已停止分发；自本版本起以本文件所述自由许可字体子集为准，发布门禁会扫描并拒绝任何含专有字体标记的产物。详见审计报告。

## 2. GUI 默认字体（egui / epaint）

`epaint_default_fonts`（egui 0.29 依赖）内置的字体资源遵循 `(MIT OR Apache-2.0) AND OFL-1.1 AND LicenseRef-UFL-1.0`。其中涉及 Ubuntu Font License 1.0 与 SIL OFL 1.1 的字体资源保持其原有许可，版权归各自作者所有。

## 3. Rust 依赖许可概览

以下为 `cargo metadata --all-features` 的统计结果（按包数量）：

| 许可表达式 | 包数 |
|------------|------|
| MIT OR Apache-2.0 | 280 |
| MIT | 156 |
| Apache-2.0 OR MIT | 43 |
| MIT/Apache-2.0 | 27 |
| Apache-2.0 | 19 |
| Unicode-3.0 | 18 |
| Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | 17 |
| Zlib OR Apache-2.0 OR MIT | 14 |
| MIT OR Apache-2.0 OR Zlib | 7 |
| Unlicense OR MIT | 6 |
| MPL-2.0 | 5 |
| ISC | 3 |
| BSD-3-Clause | 3 |
| BSL-1.0（Boost Software License） | 2 |
| Zlib | 2 |
| 其它宽松许可组合 | 若干 |

需要单独说明的依赖：

| 依赖 | 许可 | 说明 |
|------|------|------|
| `serialport` 4.9、`resvg` / `usvg` / `usvg-parser` / `usvg-tree` 0.37 | MPL-2.0 | 文件级弱 copyleft，不传染。只要不修改这些库的源文件即可按本项目许可分发；若修改，需公开被修改文件的源码。 |
| `unescaper` 0.1.8（`serialport` 的 Windows 目标依赖） | GPL-3.0/MIT（双许可） | 可选择 MIT 分支使用，不产生 GPL 传染。 |
| `epaint_default_fonts` 0.29 | MIT/Apache-2.0 + OFL-1.1 + UFL-1.0 | 字体部分按 OFL-1.1 / UFL-1.0 保留原许可。 |
| `icu_*`、`unicode-ident`、`zerovec` 等 | Unicode-3.0 | 宽松许可，需保留版权与许可声明。 |
| `r-efi` 5.3 / 6.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later | 可选择 MIT/Apache-2.0，不产生 LGPL 传染。 |

**结论**：未发现 GPL / AGPL 强制传染项；存在 MPL-2.0（文件级）与双许可项，按上表处理即可。完整依赖清单以 `Cargo.lock` 为准，各依赖的许可原文随 crate 源码分发。

## 4. 资源与图标

- `scripts/icon_preview.png` 及其生成的 `miconlab.icns` / `miconlab.ico` / `miconlab-*.png`：本项目自制图形，随本项目授权。
- 应用图标（`.icns` / `.ico` / `.png`）由上述源图生成，不包含任何第三方商标。
- 本仓库不包含第三方设备固件、厂商资料或逆向工程产物。

## 5. SIL Open Font License 1.1 全文

```text
This Font Software is licensed under the SIL Open Font License,
Version 1.1.

This license is copied below, and is also available with a FAQ at:
http://scripts.sil.org/OFL

-----------------------------------------------------------
SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007
-----------------------------------------------------------

PREAMBLE
The goals of the Open Font License (OFL) are to stimulate worldwide
development of collaborative font projects, to support the font
creation efforts of academic and linguistic communities, and to
provide a free and open framework in which fonts may be shared and
improved in partnership with others.

The OFL allows the licensed fonts to be used, studied, modified and
redistributed freely as long as they are not sold by themselves. The
fonts, including any derivative works, can be bundled, embedded,
redistributed and/or sold with any software provided that any reserved
names are not used by derivative works. The fonts and derivatives,
however, cannot be released under any other type of license. The
requirement for fonts to remain under this license does not apply to
any document created using the fonts or their derivatives.

DEFINITIONS
"Font Software" refers to the set of files released by the Copyright
Holder(s) under this license and clearly marked as such. This may
include source files, build scripts and documentation.

"Reserved Font Name" refers to any names specified as such after the
copyright statement(s).

"Original Version" refers to the collection of Font Software
components as distributed by the Copyright Holder(s).

"Modified Version" refers to any derivative made by adding to,
deleting, or substituting -- in part or in whole -- any of the
components of the Original Version, by changing formats or by porting
the Font Software to a new environment.

"Author" refers to any designer, engineer, programmer, technical
writer or other person who contributed to the Font Software.

PERMISSION & CONDITIONS
Permission is hereby granted, free of charge, to any person obtaining
a copy of the Font Software, to use, study, copy, merge, embed,
modify, redistribute, and sell modified and unmodified copies of the
Font Software, subject to the following conditions:

1) Neither the Font Software nor any of its individual components, in
Original or Modified Versions, may be sold by itself.

2) Original or Modified Versions of the Font Software may be bundled,
redistributed and/or sold with any software, provided that each copy
contains the above copyright notice and this license. These can be
included either as stand-alone text files, human-readable headers or
in the appropriate machine-readable metadata fields within text or
binary files as long as those fields can be easily viewed by the user.

3) No Modified Version of the Font Software may use the Reserved Font
Name(s) unless explicit written permission is granted by the
corresponding Copyright Holder. This restriction only applies to the
primary font name as presented to the users.

4) The name(s) of the Copyright Holder(s) or the Author(s) of the Font
Software shall not be used to promote, endorse or advertise any
Modified Version, except to acknowledge the contribution(s) of the
Copyright Holder(s) and the Author(s) or with their explicit written
permission.

5) The Font Software, modified or unmodified, in part or in whole,
must be distributed entirely under this license, and must not be
distributed under any other license. The requirement for fonts to
remain under this license does not apply to any document created using
the Font Software.

TERMINATION
This license becomes null and void if any of the above conditions are
not met.

DISCLAIMER
THE FONT SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT
OF COPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT. IN NO EVENT SHALL THE
COPYRIGHT HOLDER BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
INCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL
DAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM
OTHER DEALINGS IN THE FONT SOFTWARE.
```

## 6. 免责

本文件为项目方的合规整理，不构成法律意见。正式对外发布前建议由律师复核第三方许可与分发方式。
