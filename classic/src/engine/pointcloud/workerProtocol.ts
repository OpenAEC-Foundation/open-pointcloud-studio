import type { ParsedPointcloud } from './LASParser';

export interface ParseRequest {
  type: 'parse';
  id: string;
  extension: string;
  buffer: ArrayBuffer;
}

export interface ProgressMessage {
  type: 'progress';
  id: string;
  phase: string;
  percent: number;
}

export interface ResultMessage {
  type: 'result';
  id: string;
  data: ParsedPointcloud;
}

export interface ErrorMessage {
  type: 'error';
  id: string;
  message: string;
}

export type WorkerRequest = ParseRequest;
export type WorkerResponse = ProgressMessage | ResultMessage | ErrorMessage;
