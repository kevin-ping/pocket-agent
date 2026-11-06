# 升级版本号清单

开新 release 分支(如 `release/0.3.2`)后，把旧版本号 X 改成新版本号 Y。以 0.3.1 → 0.3.2 为例。

## 必改

| 文件 | 位置 | 说明 |
|------|------|------|
| `package.json` | 第 3 行 `"version"` | 版本号的唯一权威来源,`releases/[version].md` 按它命名 |
| `package-lock.json` | 第 3 行和第 9 行(`packages[""]`)的 `"version"` | 只改这两处，别全局替换(依赖里可能有同号) |
| `src-tauri/Cargo.toml` | `[package]` 下的 `version` | |
| `src-tauri/Cargo.lock` | `name = "pocket-agent"` 下一行的 `version` | 只改这一条;0.3.1 时有 6 个第三方 crate 也是 0.3.1 |
| `src-tauri/tauri.conf.json` | 第 3 行 `"version"` | 安装包和「关于」显示的版本 |
| `android/app/build.gradle.kts` | `versionName` 改成 Y;`versionCode` 加 1 | `versionCode` 不变大，手机无法覆盖安装 |
| `README.md` | 顶部 `> **vY** — see [releases/Y.md](releases/Y.md)` | |
| `README_AGENTS.md` | `- **Current version**: Y` | |
| `releases/Y.md` | 新建：`# Pocket Agent vY` + 空行 + `**Release Date**: TBD` | README 链接到它;正文在 auto-work FINALIZE 时补 |

## 视情况改

- `docs/windows-port.md`:写着「当前 release 分支」的地方(开发方式、验证第 1 步和第 4 步)改成新分支。

## 不改

历史记录保持原样：`docs/fresh-install-verification.md` 里带日期的验证记录、代码注释里的「0.3.1 之前」之类、`releases/` 下的旧版本说明。

## 查漏与验证

```sh
# 找残留的旧版本号(把 0\.3\.1 换成旧版本)
grep -rn --exclude-dir={node_modules,target,.git,dist,build,releases,.auto-work} '0\.3\.1' . | grep -v -E 'Cargo.lock:|package-lock'

# lock 文件与新版本一致
(cd src-tauri && cargo metadata --locked --offline --format-version 1 >/dev/null) && npm ls --depth=0 >/dev/null
```

提交信息:`chore(release): bump version to Y`。

## 发布后

旧 release 分支合并进 `main` 后：在分支最后一个 commit 上打 tag `vX` 并 push,再删除远端和本地的 `release/X` 分支。
