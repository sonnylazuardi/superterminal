/**
 * The client's own identity, as shown by `--version` and the About dialog.
 *
 * `APP_VERSION` is the release number and is bumped by hand with each release
 * (the "Bump Windows MSI to x.y.z" commits): the Windows exe is compiled with
 * no environment at all, and `bun build --compile` does not inline
 * `process.env` reads, so a checked-in constant is the only value every
 * platform actually ships. `SUPERTERMINAL_VERSION` / `SUPERTERMINAL_BUILD_ID`
 * still override it at runtime for dev builds and scripted tests.
 */

export const APP_VERSION = '0.1.17';

/** The project home, linked from the About dialog. */
export const REPO_URL = 'https://github.com/sonnylazuardi/superterminal';

export interface BuildInfo {
  version: string;
  /** Git-derived id of a packaged build; `dev` when running from source. */
  buildId: string;
}

export function resolveBuildInfo(env: Record<string, string | undefined> = process.env): BuildInfo {
  const version = env['SUPERTERMINAL_VERSION']?.trim();
  const buildId = env['SUPERTERMINAL_BUILD_ID']?.trim();
  return {
    version: version && version.length > 0 ? version : APP_VERSION,
    buildId: buildId && buildId.length > 0 ? buildId : 'dev',
  };
}

export const BUILD_INFO: BuildInfo = resolveBuildInfo();

/** One line for humans: `superterminal 0.1.14` or `superterminal 0.1.14 (0.1.14+abc123)`. */
export function describeBuild(info: BuildInfo = BUILD_INFO): string {
  return info.buildId === 'dev' || info.buildId === info.version
    ? `superterminal ${info.version}`
    : `superterminal ${info.version} (${info.buildId})`;
}
