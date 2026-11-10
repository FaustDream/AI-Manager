## 这个 PR 做了什么

<!-- 一句话。修 Bug 写「修了什么症状」，加功能写「解决了谁的什么麻烦」 -->

## 关联 Issue

Closes #

## 改动范围

- [ ] 只动了本任务直接相关的调用链，没有顺手重构无关模块
- [ ] 没有提交 `data/`、`docs/`、`tools/` 下的任何内容

## 门禁（本地已跑过）

- [ ] `npm test`
- [ ] `npm run build`
- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --all-targets --features ts -- -D warnings`
- [ ] `cargo test --lib --tests --features ts`
- [ ] 改过 Rust 端 IPC 类型：`src/lib/types/` 生成物已一起提交

## 架构约束自查

- [ ] 前端没有直接调 `invoke`，走的是 `src/lib/cmd.ts`
- [ ] 错误用的是 `AppError::new(ErrorKind, …)`，没有裸字符串
- [ ] SQL 没有逃出所属 store 模块
- [ ] 失败路径走了 `log::fail`，没有塞进 Info 日志

## 截图 / 录屏

<!-- UI 改动请附，记得脱敏：不要真实账号昵称、邮箱、手机号、token -->
