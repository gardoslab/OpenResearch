import assert from "node:assert/strict";
import test from "node:test";
import { appendToDraft, clampWidgetHeight, parseWidgetMessage } from "../src/chatWidget.ts";

const msg = (extra) => ({ orx: "widget", ...extra });

test("ignores payloads that are not widget messages", () => {
  for (const data of [null, "submit", 3, {}, { type: "submit", text: "x" }, msg({ type: "other" })]) {
    assert.equal(parseWidgetMessage(data), null);
  }
});

test("accepts plain text and trims it", () => {
  assert.deepEqual(parseWidgetMessage(msg({ type: "submit", text: "  pick B  " })), { type: "submit", text: "pick B" });
  assert.equal(parseWidgetMessage(msg({ type: "submit", text: "   " })), null);
});

test("formats values as a list and flattens newlines", () => {
  const out = parseWidgetMessage(msg({ type: "submit", values: { winner: "B", notes: "a\nb", scores: [1, 2], skip: {} } }));
  assert.deepEqual(out, { type: "submit", text: "Widget response:\n- winner: B\n- notes: a b\n- scores: 1, 2" });
});

test("rejects non-object values and caps text length", () => {
  assert.equal(parseWidgetMessage(msg({ type: "submit", values: [1] })), null);
  assert.equal(parseWidgetMessage(msg({ type: "submit", values: "x" })), null);
  assert.equal(parseWidgetMessage(msg({ type: "submit", text: "a".repeat(10_000) })).text.length, 4_000);
});

test("clamps resize heights and rejects non-finite ones", () => {
  assert.equal(clampWidgetHeight(5), 80);
  assert.equal(clampWidgetHeight(99999), 720);
  assert.deepEqual(parseWidgetMessage(msg({ type: "resize", height: 300.2 })), { type: "resize", height: 301 });
  assert.equal(parseWidgetMessage(msg({ type: "resize", height: Infinity })), null);
  assert.equal(parseWidgetMessage(msg({ type: "resize", height: "300" })), null);
});

test("appending keeps an existing draft", () => {
  assert.equal(appendToDraft("", "x"), "x");
  assert.equal(appendToDraft("hello  \n", "x"), "hello\n\nx");
});
