/**
 * The About dialog (`app.about`): the build's version and the project link.
 *
 * A Dialog like the palette — same top-centre placement, same panel — with two
 * facts and one action. Keyboard-driven the way the Menu is: a parked
 * `<input autoFocus>` owns Esc (close) and Enter (open the link; Enter only
 * ever arrives as `onSubmit`, see CommandPalette). A click elsewhere closes
 * it via `onMouseDownOutside` on the inner div (`<anchored>` has no such
 * listener).
 *
 * The link opens in the default browser through the app bridge. When nothing
 * could be launched (no `xdg-open`, sandboxed…) the URL lands in an error
 * toast so it can at least be read and retyped.
 */

import { BUILD_INFO, REPO_URL } from '../version.js';
import { dialogPlacement } from './CommandPalette.js';
import { useServices, useWorkspace } from './context.js';
import { ICONS, Icon } from './Icon.js';

const WIDTH = 420;

/** What the dialog prints under the name: the release, plus the build id when it says more. */
export function aboutLines(info: { version: string; buildId: string }): string[] {
  const lines = [`Version ${info.version}`];
  if (info.buildId !== 'dev' && info.buildId !== info.version) lines.push(`Build ${info.buildId}`);
  return lines;
}

/** The link as shown: scheme dropped, so it reads as a name rather than an address. */
export function displayUrl(url: string): string {
  return url.replace(/^https?:\/\//, '');
}

export function About() {
  const { tokens, store, commandContext } = useServices();
  const open = useWorkspace((s) => s.ui.aboutOpen);
  const windowWidth = useWorkspace((s) => s.ui.window.width);
  const vertical = useWorkspace((s) => s.ui.verticalTabs);
  const placement = dialogPlacement(windowWidth, tokens, vertical);

  if (!open) return null;

  const close = () => store.dispatch({ type: 'about.close' });
  const openRepo = () => {
    if (commandContext.app.openExternal(REPO_URL)) {
      close();
      return;
    }
    store.dispatch({ type: 'toast.push', text: `Could not open a browser: ${REPO_URL}`, kind: 'error' });
  };

  return (
    <anchored
      testId="about"
      anchor="topCenter"
      position={placement}
      style={{
        width: WIDTH,
        display: 'flex', // REQUIRED or children are blocks
        flexDirection: 'column',
        backgroundColor: tokens.bg.overlay,
        borderRadius: tokens.radius.panel,
        borderWidth: tokens.border.width,
        borderColor: tokens.border.glass,
        overflow: 'hidden', // keep rounded corners from being painted over
      }}
    >
      <div
        testId="about-body"
        onMouseDownOutside={close}
        style={{
          display: 'flex',
          flexDirection: 'column',
          padding: tokens.space.xl,
          gap: tokens.space.md,
        }}
      >
        <div
          style={{
            userSelect: 'none',
            display: 'flex',
            flexDirection: 'row',
            alignItems: 'center',
            gap: tokens.space.md,
          }}
        >
          <Icon name={ICONS.surface} size={tokens.icon.button} color={tokens.fg.primary} strokeWidth={1.75} />
          <text
            testId="about-title"
            style={{
              color: tokens.fg.primary,
              fontSize: tokens.font.paletteInput,
              fontWeight: 600,
              flexGrow: 1,
              minWidth: 0,
              overflow: 'hidden',
              whiteSpace: 'nowrap',
              textOverflow: 'ellipsis',
            }}
          >
            Superterminal
          </text>
        </div>
        {aboutLines(BUILD_INFO).map((line, i) => (
          <text
            key={line}
            testId={i === 0 ? 'about-version' : 'about-build'}
            style={{ color: tokens.fg.muted, fontSize: tokens.font.chrome }}
          >
            {line}
          </text>
        ))}
        <div
          testId="about-link"
          onClick={openRepo}
          style={{
            // Chrome text is not prose: a press here must not start a text
            // selection (gpuix `<text>` is selectable by default; this inherits).
            userSelect: 'none',
            display: 'flex',
            flexDirection: 'row',
            alignItems: 'center',
            height: tokens.strip.rowHeight,
            marginTop: tokens.space.xs,
            paddingLeft: tokens.space.lg,
            paddingRight: tokens.space.lg,
            gap: tokens.space.lg,
            borderRadius: tokens.radius.tab,
            backgroundColor: tokens.bg.glassActive,
            borderWidth: tokens.border.width,
            borderColor: tokens.accent,
            cursor: 'pointer',
            hover: { backgroundColor: tokens.bg.glassHover },
          }}
        >
          <text
            style={{
              color: tokens.accent,
              fontSize: tokens.font.chrome,
              flexGrow: 1,
              minWidth: 0,
              overflow: 'hidden',
              whiteSpace: 'nowrap',
              textOverflow: 'ellipsis',
            }}
          >
            {displayUrl(REPO_URL)}
          </text>
          <text style={{ color: tokens.fg.muted, fontSize: tokens.font.chip, flexShrink: 0 }}>
            Open in browser
          </text>
        </div>
        {/* Keyboard owner: parked in a clipped zero-height box so it never paints. */}
        <div style={{ height: 0, overflow: 'hidden' }}>
          <input
            testId="about-keys"
            autoFocus
            value=""
            style={{ height: 1, width: 1 }}
            onSubmit={openRepo}
            onKeyDown={(event) => {
              if (event.key === 'escape') close();
            }}
          />
        </div>
      </div>
    </anchored>
  );
}
