import { describe, expect, test } from 'bun:test';
import {
  FONT_SIZE_MAX,
  FONT_SIZE_MIN,
  FONT_ZOOM_MAX,
  FONT_ZOOM_MIN,
  clampFontZoom,
  zoomedFontSize,
} from './zoom.js';

describe('clampFontZoom', () => {
  test('keeps a delta inside the bounds and rounds it', () => {
    expect(clampFontZoom(0)).toBe(0);
    expect(clampFontZoom(3)).toBe(3);
    expect(clampFontZoom(2.6)).toBe(3);
    expect(clampFontZoom(FONT_ZOOM_MAX + 5)).toBe(FONT_ZOOM_MAX);
    expect(clampFontZoom(FONT_ZOOM_MIN - 5)).toBe(FONT_ZOOM_MIN);
  });

  test('garbage is no zoom', () => {
    expect(clampFontZoom(Number.NaN)).toBe(0);
    expect(clampFontZoom(Number.POSITIVE_INFINITY)).toBe(0);
  });
});

describe('zoomedFontSize', () => {
  test('adds the delta to the configured size', () => {
    expect(zoomedFontSize(13, 0)).toBe(13);
    expect(zoomedFontSize(13, 2)).toBe(15);
    expect(zoomedFontSize(13, -3)).toBe(10);
  });

  test('never asks the grid for an unreadable or absurd size', () => {
    expect(zoomedFontSize(8, FONT_ZOOM_MIN)).toBe(FONT_SIZE_MIN);
    expect(zoomedFontSize(90, FONT_ZOOM_MAX)).toBe(FONT_SIZE_MAX);
  });

  test('a broken base size falls back to a sane default', () => {
    expect(zoomedFontSize(Number.NaN, 0)).toBe(13);
    expect(zoomedFontSize(0, 1)).toBe(14);
  });
});
