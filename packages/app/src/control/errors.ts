import type { ErrorBody, ErrorCode } from '@superterminal/protocol-ts';

/** An `{"t":"err"}` response from the server (02 §3.1). */
export class ControlError extends Error {
  readonly code: ErrorCode;
  readonly data?: unknown;
  constructor(body: ErrorBody) {
    super(body.message);
    this.name = 'ControlError';
    this.code = body.code;
    this.data = body.data;
  }
}

/** No response arrived inside the per-request budget. */
export class TimeoutError extends Error {
  readonly requestType: string;
  readonly timeoutMs: number;
  constructor(requestType: string, timeoutMs: number) {
    super(`${requestType} timed out after ${timeoutMs} ms`);
    this.name = 'TimeoutError';
    this.requestType = requestType;
    this.timeoutMs = timeoutMs;
  }
}

/** The socket went away (or was closed) with the request still outstanding. */
export class DisconnectedError extends Error {
  constructor(message = 'control connection lost') {
    super(message);
    this.name = 'DisconnectedError';
  }
}

/** The server refused the handshake — major version mismatch etc. (Q31). */
export class VersionMismatchError extends Error {
  readonly reason: string;
  readonly serverVersion: string;
  constructor(reason: string, message: string, serverVersion: string) {
    super(message);
    this.name = 'VersionMismatchError';
    this.reason = reason;
    this.serverVersion = serverVersion;
  }
}

/** A peer sent something that is not valid NDJSON control traffic. */
export class ProtocolError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ProtocolError';
  }
}
