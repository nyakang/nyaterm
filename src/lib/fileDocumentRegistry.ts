import { useSyncExternalStore } from "react";
import type { FileDocumentSnapshot } from "@/types/global";

export type FileDocumentSaveResult = "saved" | "conflict";

export interface FileDocumentController {
  save: (force?: boolean) => Promise<FileDocumentSaveResult>;
  discard: () => void;
  getSnapshot?: () => FileDocumentSnapshot;
}

export interface FileDocumentRuntimeState {
  dirty: boolean;
  saving: boolean;
}

const CLEAN_STATE: FileDocumentRuntimeState = { dirty: false, saving: false };
const controllers = new Map<string, FileDocumentController>();
let states = new Map<string, FileDocumentRuntimeState>();
const listeners = new Set<() => void>();

function emitChange() {
  for (const listener of listeners) listener();
}

export function registerFileDocument(
  paneId: string,
  controller: FileDocumentController,
  state: FileDocumentRuntimeState = CLEAN_STATE,
) {
  controllers.set(paneId, controller);
  states = new Map(states).set(paneId, state);
  emitChange();

  return () => {
    if (controllers.get(paneId) !== controller) return;
    controllers.delete(paneId);
    const next = new Map(states);
    next.delete(paneId);
    states = next;
    emitChange();
  };
}

export function updateFileDocumentState(paneId: string, state: FileDocumentRuntimeState) {
  const current = states.get(paneId);
  if (current?.dirty === state.dirty && current.saving === state.saving) return;
  states = new Map(states).set(paneId, state);
  emitChange();
}

export function getFileDocumentController(paneId: string) {
  return controllers.get(paneId) ?? null;
}

export function getDirtyFileDocumentIds(paneIds?: Iterable<string>) {
  const candidates = paneIds ?? states.keys();
  return [...candidates].filter((paneId) => states.get(paneId)?.dirty);
}

export async function saveFileDocuments(paneIds: Iterable<string>) {
  for (const paneId of paneIds) {
    const controller = controllers.get(paneId);
    if (controller && (await controller.save(false)) !== "saved") return false;
  }
  return true;
}

export function discardFileDocuments(paneIds: Iterable<string>) {
  for (const paneId of paneIds) controllers.get(paneId)?.discard();
}

export function getFileDocumentState(paneId: string) {
  return states.get(paneId) ?? CLEAN_STATE;
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function getStatesSnapshot() {
  return states;
}

export function useFileDocumentState(paneId: string) {
  const snapshot = useSyncExternalStore(subscribe, getStatesSnapshot, getStatesSnapshot);
  return snapshot.get(paneId) ?? CLEAN_STATE;
}

export function useFileDocumentStates() {
  return useSyncExternalStore(subscribe, getStatesSnapshot, getStatesSnapshot);
}
