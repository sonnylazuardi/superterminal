/** `superterminal` argv parsing (05 §1). Pure, so it is unit-testable. */

export interface Argv {
  version: boolean;
  help: boolean;
  socket?: string;
  config?: string;
  noSpawn: boolean;
  foregroundServer: boolean;
  unknown: string[];
}

export function parseArgv(args: string[]): Argv {
  const out: Argv = {
    version: false,
    help: false,
    noSpawn: false,
    foregroundServer: false,
    unknown: [],
  };
  for (let i = 0; i < args.length; i++) {
    const arg = args[i]!;
    const [flag, inline] = splitInline(arg);
    switch (flag) {
      case '--version':
      case '-v':
        out.version = true;
        break;
      case '--help':
      case '-h':
        out.help = true;
        break;
      case '--no-spawn':
        out.noSpawn = true;
        break;
      case '--foreground-server':
        out.foregroundServer = true;
        break;
      case '--socket': {
        const value = inline ?? args[++i];
        if (value !== undefined) out.socket = value;
        break;
      }
      case '--tcp': {
        // The Windows/WSL transport: `--tcp 127.0.0.1:7171` is shorthand for
        // `--socket tcp://127.0.0.1:7171`, so one plumbing path serves both.
        const value = inline ?? args[++i];
        if (value !== undefined) out.socket = `tcp://${value}`;
        break;
      }
      case '--config': {
        const value = inline ?? args[++i];
        if (value !== undefined) out.config = value;
        break;
      }
      default:
        out.unknown.push(arg);
    }
  }
  return out;
}

function splitInline(arg: string): [string, string | undefined] {
  const eq = arg.indexOf('=');
  if (!arg.startsWith('--') || eq < 0) return [arg, undefined];
  return [arg.slice(0, eq), arg.slice(eq + 1)];
}

export const USAGE = `superterminal — a GPU terminal multiplexer

  --socket <path>       control socket (default: $XDG_RUNTIME_DIR/superterminal/control.sock)
  --tcp <host:port>     control + data over TCP (Windows client, WSL server)
  --config <path>       config.toml to load
  --no-spawn            never start superterminald; fail if none is running
  --foreground-server   run the daemon in the foreground (dev)
  -v, --version         print version and exit
  -h, --help            this text
`;
