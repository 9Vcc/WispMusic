<div align="center">
  <h1>WispMusic</h1>

  <img width="128" height="128" alt="WispMusic" src="./src-tauri/icons/icon.png">
</div>

<div align="center">
  <p>简洁美观的第三方网易云音乐客户端</p>
</div>

<div align="center">
  <img src="https://img.shields.io/badge/Framework-Tauri-FFC131" alt="Framework: Tauri">
  <img src="https://img.shields.io/badge/Platform-Windows-blue" alt="Platform: Windows">
  <a href="https://www.gnu.org/licenses/agpl-3.0">
    <img src="https://img.shields.io/badge/License-AGPL_3.0-blue.svg" alt="License: AGPL 3.0">
  </a>
</div>


## 📦 安装使用

前往 [Releases](https://github.com/9Vcc/WYMusic/releases) 页面下载最新版安装包（`WispMusic_x64-setup.exe`），双击按向导安装即可。

> 系统要求：Windows 10/11（首次运行如缺少 WebView2 运行时会自动引导安装）

## 🛠 开发

在开始之前，请确保你的开发环境已安装 [Rust](https://www.rust-lang.org/tools/install)、[Node.js](https://nodejs.org/) 和 [pnpm](https://pnpm.io/installation)。

### 1. 环境要求

- **Node.js**: >= 20
- **pnpm**: >= 9
- **Rust**: MSVC 工具链
- **Windows 依赖**: C++ 生成工具和 Edge WebView2 运行时

### 2. 本地运行

```bash
# 1. 克隆项目
git clone https://github.com/9Vcc/WYMusic.git
cd WYMusic

# 2. 安装依赖
pnpm install

# 3. 启动开发环境
pnpm tdev

# 4. 构建
pnpm tbuild
```

### 3. 常用命令

```bash
pnpm test    # 运行单元测试（Vitest）
pnpm build   # 仅构建前端（TypeScript 检查 + Vite 打包）
```

版本变更记录见 [CHANGELOG.md](./CHANGELOG.md)。

## 🧰 技术栈

- **核心框架**: React 19, TypeScript, Rust, Tauri 2.0, Vite 7
- **音频**: Rodio + 自研 DSP
- **UI**: TailwindCSS v4, shadcn/ui, Fluent Icons
- **视觉与动效**: Framer Motion, Three.js, React Three Fiber, Shaders React
- **状态与数据流**: Zustand, SWR, IndexedDB
- **代码规范与工具链**: Biome, Vitest, pnpm

## 🤝 项目来源

本项目基于 [Yee Music](https://github.com/1sen3/YeeMusic) 继续开发，保留原项目的播放器能力、版权声明和开源许可。

感谢以下项目：

- [Tauri](https://github.com/tauri-apps/tauri)
- [NeteaseCloudMusicAPI Enhanced](https://github.com/neteasecloudmusicapienhanced/api-enhanced)
- [Howler.js](https://github.com/goldfire/howler.js)
- [Zustand](https://github.com/pmndrs/zustand)
- [AMLL](https://github.com/amll-dev/applemusic-like-lyrics)
- [YeeMusic](https://github.com/1sen3/YeeMusic/tree/main)

## 📄 声明与许可

版权所有 (C) 2026 9Vcc 及 WispMusic 贡献者。本项目基于 [YeeMusic](https://github.com/1sen3/YeeMusic/tree/main) 二次开发，上游项目版权归 1sen3 所有。二次开发代码多数为 Vibe Coding 产物，仅用于学习和技术交流。

本项目完整源代码公开于本仓库，遵循 [GNU Affero General Public License v3.0](./LICENSE) 发行：

- **随附源码**：以任何方式分发本程序（含安装包）时，须保留版权与许可声明，并同时提供对应的完整源代码，或注明其获取途径（即本仓库地址）；
- **网络服务条款**：依据 AGPL-3.0 第 13 条，将本程序的修改版本作为网络服务向用户提供时，同样须向服务使用者公开对应的完整源代码。

本项目使用的第三方组件遵循其自身许可条款（MIT / BSD / Apache-2.0 等），分发时应保留各组件的版权与许可声明。

以上说明为善意免责声明，不构成对 AGPL-3.0 许可条款的额外限制，许可条款以 [AGPL-3.0 全文](./LICENSE) 为准。音乐数据及相关内容来自第三方服务，版权归原权利人所有，请勿用于违法或侵权用途。
