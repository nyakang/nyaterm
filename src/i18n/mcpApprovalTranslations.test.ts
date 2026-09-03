import { expect, it } from "vitest";
import en from "./locales/en.json";
import ko from "./locales/ko.json";
import zhCN from "./locales/zh-CN.json";
import zhTW from "./locales/zh-TW.json";

it("describes MCP approval grants as connection-scoped in every locale", () => {
  expect(en.ai.externalMcpAllowSession).toBe("Allow for this connection");
  expect(zhCN.ai.externalMcpAllowSession).toBe("本次连接中允许");
  expect(zhTW.ai.externalMcpAllowSession).toBe("此連線期間允許");
  expect(ko.ai.externalMcpAllowSession).toBe("이 연결에서 허용");
});

it("labels both saved connections and sessions as MCP approval targets", () => {
  expect(en.ai.externalMcpTarget).toBe("Target");
  expect(zhCN.ai.externalMcpTarget).toBe("目标");
  expect(zhTW.ai.externalMcpTarget).toBe("目標");
  expect(ko.ai.externalMcpTarget).toBe("대상");
});

it("uses explicit names for all AI permission modes in every locale", () => {
  expect([
    en.ai.permissionObserver,
    en.ai.permissionConfirm,
    en.ai.permissionAuto,
    en.ai.permissionFullAccess,
  ]).toEqual(["Read-only", "Always confirm", "Safe auto", "Full access"]);
  expect([
    zhCN.ai.permissionObserver,
    zhCN.ai.permissionConfirm,
    zhCN.ai.permissionAuto,
    zhCN.ai.permissionFullAccess,
  ]).toEqual(["只读", "每次确认", "安全自动", "完全权限"]);
  expect([
    zhTW.ai.permissionObserver,
    zhTW.ai.permissionConfirm,
    zhTW.ai.permissionAuto,
    zhTW.ai.permissionFullAccess,
  ]).toEqual(["唯讀", "每次確認", "安全自動", "完全權限"]);
  expect([
    ko.ai.permissionObserver,
    ko.ai.permissionConfirm,
    ko.ai.permissionAuto,
    ko.ai.permissionFullAccess,
  ]).toEqual(["읽기 전용", "매번 확인", "안전 자동", "전체 권한"]);
});

it("localizes external agent MCP fallback and startup errors", () => {
  for (const locale of [en, zhCN, zhTW, ko]) {
    expect(locale.ai.codexMcpFallback).toBeTruthy();
    expect(locale.ai.claudeMcpUnavailable).toContain("{{reason}}");
  }
});
