import { SystemInstructionType } from '@solana/web3.js';
import { IInstructionInputData, InstructionType } from '.';
import * as BufferLayout from '@solana/buffer-layout';
import * as Layout from './layout';
import { u64 } from './solana-bigint';

type SystemInstructionInputData = {
  AdvanceNonceAccount: IInstructionInputData;
  Allocate: IInstructionInputData & {
    space: number;
  };
  AllocateWithSeed: IInstructionInputData & {
    base: Uint8Array;
    programId: Uint8Array;
    seed: string;
    space: number;
  };
  Assign: IInstructionInputData & {
    programId: Uint8Array;
  };
  AssignWithSeed: IInstructionInputData & {
    base: Uint8Array;
    seed: string;
    programId: Uint8Array;
  };
  AuthorizeNonceAccount: IInstructionInputData & {
    authorized: Uint8Array;
  };
  Create: IInstructionInputData & {
    lamports: number;
    programId: Uint8Array;
    space: number;
  };
  CreateWithSeed: IInstructionInputData & {
    base: Uint8Array;
    lamports: number;
    programId: Uint8Array;
    seed: string;
    space: number;
  };
  InitializeNonceAccount: IInstructionInputData & {
    authorized: Uint8Array;
  };
  Transfer: IInstructionInputData & {
    lamports: bigint;
  };
  TransferWithSeed: IInstructionInputData & {
    lamports: bigint;
    programId: Uint8Array;
    seed: string;
  };
  WithdrawNonceAccount: IInstructionInputData & {
    lamports: number;
  };
  UpgradeNonceAccount: IInstructionInputData;
};

export const SYSTEM_INSTRUCTION_LAYOUTS = Object.freeze<{
  [Instruction in SystemInstructionType]: InstructionType<SystemInstructionInputData[Instruction]>;
}>({
  Create: {
    index: 0,
    layout: BufferLayout.struct<SystemInstructionInputData['Create']>([
      BufferLayout.u32('instruction'),
      BufferLayout.ns64('lamports'),
      BufferLayout.ns64('space'),
      Layout.publicKey('programId'),
    ]),
  },
  Assign: {
    index: 1,
    layout: BufferLayout.struct<SystemInstructionInputData['Assign']>([
      BufferLayout.u32('instruction'),
      Layout.publicKey('programId'),
    ]),
  },
  Transfer: {
    index: 2,
    layout: BufferLayout.struct<SystemInstructionInputData['Transfer']>([
      BufferLayout.u32('instruction'),
      u64('lamports'),
    ]),
  },
  CreateWithSeed: {
    index: 3,
    layout: BufferLayout.struct<SystemInstructionInputData['CreateWithSeed']>([
      BufferLayout.u32('instruction'),
      Layout.publicKey('base'),
      Layout.rustString('seed'),
      BufferLayout.ns64('lamports'),
      BufferLayout.ns64('space'),
      Layout.publicKey('programId'),
    ]),
  },
  AdvanceNonceAccount: {
    index: 4,
    layout: BufferLayout.struct<SystemInstructionInputData['AdvanceNonceAccount']>([BufferLayout.u32('instruction')]),
  },
  WithdrawNonceAccount: {
    index: 5,
    layout: BufferLayout.struct<SystemInstructionInputData['WithdrawNonceAccount']>([
      BufferLayout.u32('instruction'),
      BufferLayout.ns64('lamports'),
    ]),
  },
  InitializeNonceAccount: {
    index: 6,
    layout: BufferLayout.struct<SystemInstructionInputData['InitializeNonceAccount']>([
      BufferLayout.u32('instruction'),
      Layout.publicKey('authorized'),
    ]),
  },
  AuthorizeNonceAccount: {
    index: 7,
    layout: BufferLayout.struct<SystemInstructionInputData['AuthorizeNonceAccount']>([
      BufferLayout.u32('instruction'),
      Layout.publicKey('authorized'),
    ]),
  },
  Allocate: {
    index: 8,
    layout: BufferLayout.struct<SystemInstructionInputData['Allocate']>([
      BufferLayout.u32('instruction'),
      BufferLayout.ns64('space'),
    ]),
  },
  AllocateWithSeed: {
    index: 9,
    layout: BufferLayout.struct<SystemInstructionInputData['AllocateWithSeed']>([
      BufferLayout.u32('instruction'),
      Layout.publicKey('base'),
      Layout.rustString('seed'),
      BufferLayout.ns64('space'),
      Layout.publicKey('programId'),
    ]),
  },
  AssignWithSeed: {
    index: 10,
    layout: BufferLayout.struct<SystemInstructionInputData['AssignWithSeed']>([
      BufferLayout.u32('instruction'),
      Layout.publicKey('base'),
      Layout.rustString('seed'),
      Layout.publicKey('programId'),
    ]),
  },
  TransferWithSeed: {
    index: 11,
    layout: BufferLayout.struct<SystemInstructionInputData['TransferWithSeed']>([
      BufferLayout.u32('instruction'),
      u64('lamports'),
      Layout.rustString('seed'),
      Layout.publicKey('programId'),
    ]),
  },
  UpgradeNonceAccount: {
    index: 12,
    layout: BufferLayout.struct<SystemInstructionInputData['UpgradeNonceAccount']>([BufferLayout.u32('instruction')]),
  },
});
