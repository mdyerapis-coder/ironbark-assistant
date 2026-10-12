import type {
  ContextPackage, HarnessDescriptor, OpenSessionInput, PermissionAnswer,
  SessionRef, TurnInput, WorkerEvent,
} from "../../../contracts/src/generated";

export interface HarnessAdapter {
  describe(): Promise<HarnessDescriptor>;
  open(input: OpenSessionInput): Promise<SessionRef>;
  turn(session: SessionRef, input: TurnInput): AsyncIterable<WorkerEvent>;
  answerPermission(input: PermissionAnswer): Promise<void>;
  interrupt(session: SessionRef): Promise<void>;
  exportContext(session: SessionRef): Promise<ContextPackage>;
  close(session: SessionRef): Promise<void>;
}
