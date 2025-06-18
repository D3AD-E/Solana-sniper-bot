// package: shredstream
// file: shredstream/shredstream.proto

/* tslint:disable */
/* eslint-disable */

import * as grpc from "@grpc/grpc-js";
import * as shredstream_shredstream_pb from "../shredstream/shredstream_pb";
import * as google_protobuf_timestamp_pb from "google-protobuf/google/protobuf/timestamp_pb";

interface IShredstreamProxyService extends grpc.ServiceDefinition<grpc.UntypedServiceImplementation> {
    subscribeEntries: IShredstreamProxyService_ISubscribeEntries;
}

interface IShredstreamProxyService_ISubscribeEntries extends grpc.MethodDefinition<shredstream_shredstream_pb.SubscribeEntriesRequest, shredstream_shredstream_pb.Entry> {
    path: "/shredstream.ShredstreamProxy/SubscribeEntries";
    requestStream: false;
    responseStream: true;
    requestSerialize: grpc.serialize<shredstream_shredstream_pb.SubscribeEntriesRequest>;
    requestDeserialize: grpc.deserialize<shredstream_shredstream_pb.SubscribeEntriesRequest>;
    responseSerialize: grpc.serialize<shredstream_shredstream_pb.Entry>;
    responseDeserialize: grpc.deserialize<shredstream_shredstream_pb.Entry>;
}

export const ShredstreamProxyService: IShredstreamProxyService;

export interface IShredstreamProxyServer extends grpc.UntypedServiceImplementation {
    subscribeEntries: grpc.handleServerStreamingCall<shredstream_shredstream_pb.SubscribeEntriesRequest, shredstream_shredstream_pb.Entry>;
}

export interface IShredstreamProxyClient {
    subscribeEntries(request: shredstream_shredstream_pb.SubscribeEntriesRequest, options?: Partial<grpc.CallOptions>): grpc.ClientReadableStream<shredstream_shredstream_pb.Entry>;
    subscribeEntries(request: shredstream_shredstream_pb.SubscribeEntriesRequest, metadata?: grpc.Metadata, options?: Partial<grpc.CallOptions>): grpc.ClientReadableStream<shredstream_shredstream_pb.Entry>;
}

export class ShredstreamProxyClient extends grpc.Client implements IShredstreamProxyClient {
    constructor(address: string, credentials: grpc.ChannelCredentials, options?: Partial<grpc.ClientOptions>);
    public subscribeEntries(request: shredstream_shredstream_pb.SubscribeEntriesRequest, options?: Partial<grpc.CallOptions>): grpc.ClientReadableStream<shredstream_shredstream_pb.Entry>;
    public subscribeEntries(request: shredstream_shredstream_pb.SubscribeEntriesRequest, metadata?: grpc.Metadata, options?: Partial<grpc.CallOptions>): grpc.ClientReadableStream<shredstream_shredstream_pb.Entry>;
}
