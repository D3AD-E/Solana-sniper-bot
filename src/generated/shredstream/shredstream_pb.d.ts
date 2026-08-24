// package: shredstream
// file: shredstream.proto

/* tslint:disable */
/* eslint-disable */

import * as jspb from "google-protobuf";
import * as google_protobuf_timestamp_pb from "google-protobuf/google/protobuf/timestamp_pb";

export class Header extends jspb.Message { 

    hasTs(): boolean;
    clearTs(): void;
    getTs(): google_protobuf_timestamp_pb.Timestamp | undefined;
    setTs(value?: google_protobuf_timestamp_pb.Timestamp): Header;

    serializeBinary(): Uint8Array;
    toObject(includeInstance?: boolean): Header.AsObject;
    static toObject(includeInstance: boolean, msg: Header): Header.AsObject;
    static extensions: {[key: number]: jspb.ExtensionFieldInfo<jspb.Message>};
    static extensionsBinary: {[key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message>};
    static serializeBinaryToWriter(message: Header, writer: jspb.BinaryWriter): void;
    static deserializeBinary(bytes: Uint8Array): Header;
    static deserializeBinaryFromReader(message: Header, reader: jspb.BinaryReader): Header;
}

export namespace Header {
    export type AsObject = {
        ts?: google_protobuf_timestamp_pb.Timestamp.AsObject,
    }
}

export class SubscribeEntriesRequest extends jspb.Message { 

    serializeBinary(): Uint8Array;
    toObject(includeInstance?: boolean): SubscribeEntriesRequest.AsObject;
    static toObject(includeInstance: boolean, msg: SubscribeEntriesRequest): SubscribeEntriesRequest.AsObject;
    static extensions: {[key: number]: jspb.ExtensionFieldInfo<jspb.Message>};
    static extensionsBinary: {[key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message>};
    static serializeBinaryToWriter(message: SubscribeEntriesRequest, writer: jspb.BinaryWriter): void;
    static deserializeBinary(bytes: Uint8Array): SubscribeEntriesRequest;
    static deserializeBinaryFromReader(message: SubscribeEntriesRequest, reader: jspb.BinaryReader): SubscribeEntriesRequest;
}

export namespace SubscribeEntriesRequest {
    export type AsObject = {
    }
}

export class Entry extends jspb.Message { 
    getSlot(): number;
    setSlot(value: number): Entry;
    getEntries(): Uint8Array | string;
    getEntries_asU8(): Uint8Array;
    getEntries_asB64(): string;
    setEntries(value: Uint8Array | string): Entry;

    serializeBinary(): Uint8Array;
    toObject(includeInstance?: boolean): Entry.AsObject;
    static toObject(includeInstance: boolean, msg: Entry): Entry.AsObject;
    static extensions: {[key: number]: jspb.ExtensionFieldInfo<jspb.Message>};
    static extensionsBinary: {[key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message>};
    static serializeBinaryToWriter(message: Entry, writer: jspb.BinaryWriter): void;
    static deserializeBinary(bytes: Uint8Array): Entry;
    static deserializeBinaryFromReader(message: Entry, reader: jspb.BinaryReader): Entry;
}

export namespace Entry {
    export type AsObject = {
        slot: number,
        entries: Uint8Array | string,
    }
}

export class SubscribePumpCreatesRequest extends jspb.Message { 

    serializeBinary(): Uint8Array;
    toObject(includeInstance?: boolean): SubscribePumpCreatesRequest.AsObject;
    static toObject(includeInstance: boolean, msg: SubscribePumpCreatesRequest): SubscribePumpCreatesRequest.AsObject;
    static extensions: {[key: number]: jspb.ExtensionFieldInfo<jspb.Message>};
    static extensionsBinary: {[key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message>};
    static serializeBinaryToWriter(message: SubscribePumpCreatesRequest, writer: jspb.BinaryWriter): void;
    static deserializeBinary(bytes: Uint8Array): SubscribePumpCreatesRequest;
    static deserializeBinaryFromReader(message: SubscribePumpCreatesRequest, reader: jspb.BinaryReader): SubscribePumpCreatesRequest;
}

export namespace SubscribePumpCreatesRequest {
    export type AsObject = {
    }
}

export class PumpCreate extends jspb.Message { 
    getSlot(): number;
    setSlot(value: number): PumpCreate;
    getMint(): Uint8Array | string;
    getMint_asU8(): Uint8Array;
    getMint_asB64(): string;
    setMint(value: Uint8Array | string): PumpCreate;
    getBondingCurve(): Uint8Array | string;
    getBondingCurve_asU8(): Uint8Array;
    getBondingCurve_asB64(): string;
    setBondingCurve(value: Uint8Array | string): PumpCreate;
    getAssociatedBondingCurve(): Uint8Array | string;
    getAssociatedBondingCurve_asU8(): Uint8Array;
    getAssociatedBondingCurve_asB64(): string;
    setAssociatedBondingCurve(value: Uint8Array | string): PumpCreate;
    getCreator(): Uint8Array | string;
    getCreator_asU8(): Uint8Array;
    getCreator_asB64(): string;
    setCreator(value: Uint8Array | string): PumpCreate;
    getUser(): Uint8Array | string;
    getUser_asU8(): Uint8Array;
    getUser_asB64(): string;
    setUser(value: Uint8Array | string): PumpCreate;
    getTokenProgram(): Uint8Array | string;
    getTokenProgram_asU8(): Uint8Array;
    getTokenProgram_asB64(): string;
    setTokenProgram(value: Uint8Array | string): PumpCreate;
    getDevBuyLamports(): number;
    setDevBuyLamports(value: number): PumpCreate;
    getSignature(): Uint8Array | string;
    getSignature_asU8(): Uint8Array;
    getSignature_asB64(): string;
    setSignature(value: Uint8Array | string): PumpCreate;
    getIsV2(): boolean;
    setIsV2(value: boolean): PumpCreate;
    getDetectedAtMicros(): number;
    setDetectedAtMicros(value: number): PumpCreate;

    serializeBinary(): Uint8Array;
    toObject(includeInstance?: boolean): PumpCreate.AsObject;
    static toObject(includeInstance: boolean, msg: PumpCreate): PumpCreate.AsObject;
    static extensions: {[key: number]: jspb.ExtensionFieldInfo<jspb.Message>};
    static extensionsBinary: {[key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message>};
    static serializeBinaryToWriter(message: PumpCreate, writer: jspb.BinaryWriter): void;
    static deserializeBinary(bytes: Uint8Array): PumpCreate;
    static deserializeBinaryFromReader(message: PumpCreate, reader: jspb.BinaryReader): PumpCreate;
}

export namespace PumpCreate {
    export type AsObject = {
        slot: number,
        mint: Uint8Array | string,
        bondingCurve: Uint8Array | string,
        associatedBondingCurve: Uint8Array | string,
        creator: Uint8Array | string,
        user: Uint8Array | string,
        tokenProgram: Uint8Array | string,
        devBuyLamports: number,
        signature: Uint8Array | string,
        isV2: boolean,
        detectedAtMicros: number,
    }
}

export class SubscribeFillsRequest extends jspb.Message { 

    serializeBinary(): Uint8Array;
    toObject(includeInstance?: boolean): SubscribeFillsRequest.AsObject;
    static toObject(includeInstance: boolean, msg: SubscribeFillsRequest): SubscribeFillsRequest.AsObject;
    static extensions: {[key: number]: jspb.ExtensionFieldInfo<jspb.Message>};
    static extensionsBinary: {[key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message>};
    static serializeBinaryToWriter(message: SubscribeFillsRequest, writer: jspb.BinaryWriter): void;
    static deserializeBinary(bytes: Uint8Array): SubscribeFillsRequest;
    static deserializeBinaryFromReader(message: SubscribeFillsRequest, reader: jspb.BinaryReader): SubscribeFillsRequest;
}

export namespace SubscribeFillsRequest {
    export type AsObject = {
    }
}

export class Fill extends jspb.Message { 
    getSlot(): number;
    setSlot(value: number): Fill;
    getMint(): Uint8Array | string;
    getMint_asU8(): Uint8Array;
    getMint_asB64(): string;
    setMint(value: Uint8Array | string): Fill;
    getTokenAccount(): Uint8Array | string;
    getTokenAccount_asU8(): Uint8Array;
    getTokenAccount_asB64(): string;
    setTokenAccount(value: Uint8Array | string): Fill;
    getSeed(): string;
    setSeed(value: string): Fill;
    getTokenProgram(): Uint8Array | string;
    getTokenProgram_asU8(): Uint8Array;
    getTokenProgram_asB64(): string;
    setTokenProgram(value: Uint8Array | string): Fill;
    getBondingCurve(): Uint8Array | string;
    getBondingCurve_asU8(): Uint8Array;
    getBondingCurve_asB64(): string;
    setBondingCurve(value: Uint8Array | string): Fill;
    getAssociatedBondingCurve(): Uint8Array | string;
    getAssociatedBondingCurve_asU8(): Uint8Array;
    getAssociatedBondingCurve_asB64(): string;
    setAssociatedBondingCurve(value: Uint8Array | string): Fill;
    getCreator(): Uint8Array | string;
    getCreator_asU8(): Uint8Array;
    getCreator_asB64(): string;
    setCreator(value: Uint8Array | string): Fill;
    getAmount(): number;
    setAmount(value: number): Fill;
    getMaxSolCost(): number;
    setMaxSolCost(value: number): Fill;
    getFiredAtMicros(): number;
    setFiredAtMicros(value: number): Fill;

    serializeBinary(): Uint8Array;
    toObject(includeInstance?: boolean): Fill.AsObject;
    static toObject(includeInstance: boolean, msg: Fill): Fill.AsObject;
    static extensions: {[key: number]: jspb.ExtensionFieldInfo<jspb.Message>};
    static extensionsBinary: {[key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message>};
    static serializeBinaryToWriter(message: Fill, writer: jspb.BinaryWriter): void;
    static deserializeBinary(bytes: Uint8Array): Fill;
    static deserializeBinaryFromReader(message: Fill, reader: jspb.BinaryReader): Fill;
}

export namespace Fill {
    export type AsObject = {
        slot: number,
        mint: Uint8Array | string,
        tokenAccount: Uint8Array | string,
        seed: string,
        tokenProgram: Uint8Array | string,
        bondingCurve: Uint8Array | string,
        associatedBondingCurve: Uint8Array | string,
        creator: Uint8Array | string,
        amount: number,
        maxSolCost: number,
        firedAtMicros: number,
    }
}
