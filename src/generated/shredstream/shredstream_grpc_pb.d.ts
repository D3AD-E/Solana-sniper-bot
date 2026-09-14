// package: shredstream
// file: shredstream/shredstream.proto

/* tslint:disable */
/* eslint-disable */

import * as grpc from "@grpc/grpc-js";
import * as shredstream_shredstream_pb from "../shredstream/shredstream_pb";
import * as google_protobuf_timestamp_pb from "google-protobuf/google/protobuf/timestamp_pb";

interface IShredstreamProxyService extends grpc.ServiceDefinition<grpc.UntypedServiceImplementation> {
    subscribeEntries: IShredstreamProxyService_ISubscribeEntries;
    subscribePumpCreates: IShredstreamProxyService_ISubscribePumpCreates;
    subscribeFills: IShredstreamProxyService_ISubscribeFills;
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
interface IShredstreamProxyService_ISubscribePumpCreates extends grpc.MethodDefinition<shredstream_shredstream_pb.SubscribePumpCreatesRequest, shredstream_shredstream_pb.PumpCreate> {
    path: "/shredstream.ShredstreamProxy/SubscribePumpCreates";
    requestStream: false;
    responseStream: true;
    requestSerialize: grpc.serialize<shredstream_shredstream_pb.SubscribePumpCreatesRequest>;
    requestDeserialize: grpc.deserialize<shredstream_shredstream_pb.SubscribePumpCreatesRequest>;
    responseSerialize: grpc.serialize<shredstream_shredstream_pb.PumpCreate>;
    responseDeserialize: grpc.deserialize<shredstream_shredstream_pb.PumpCreate>;
}
interface IShredstreamProxyService_ISubscribeFills extends grpc.MethodDefinition<shredstream_shredstream_pb.SubscribeFillsRequest, shredstream_shredstream_pb.Fill> {
    path: "/shredstream.ShredstreamProxy/SubscribeFills";
    requestStream: false;
    responseStream: true;
    requestSerialize: grpc.serialize<shredstream_shredstream_pb.SubscribeFillsRequest>;
    requestDeserialize: grpc.deserialize<shredstream_shredstream_pb.SubscribeFillsRequest>;
    responseSerialize: grpc.serialize<shredstream_shredstream_pb.Fill>;
    responseDeserialize: grpc.deserialize<shredstream_shredstream_pb.Fill>;
}

export const ShredstreamProxyService: IShredstreamProxyService;

export interface IShredstreamProxyServer extends grpc.UntypedServiceImplementation {
    subscribeEntries: grpc.handleServerStreamingCall<shredstream_shredstream_pb.SubscribeEntriesRequest, shredstream_shredstream_pb.Entry>;
    subscribePumpCreates: grpc.handleServerStreamingCall<shredstream_shredstream_pb.SubscribePumpCreatesRequest, shredstream_shredstream_pb.PumpCreate>;
    subscribeFills: grpc.handleServerStreamingCall<shredstream_shredstream_pb.SubscribeFillsRequest, shredstream_shredstream_pb.Fill>;
}

export interface IShredstreamProxyClient {
    subscribeEntries(request: shredstream_shredstream_pb.SubscribeEntriesRequest, options?: Partial<grpc.CallOptions>): grpc.ClientReadableStream<shredstream_shredstream_pb.Entry>;
    subscribeEntries(request: shredstream_shredstream_pb.SubscribeEntriesRequest, metadata?: grpc.Metadata, options?: Partial<grpc.CallOptions>): grpc.ClientReadableStream<shredstream_shredstream_pb.Entry>;
    subscribePumpCreates(request: shredstream_shredstream_pb.SubscribePumpCreatesRequest, options?: Partial<grpc.CallOptions>): grpc.ClientReadableStream<shredstream_shredstream_pb.PumpCreate>;
    subscribePumpCreates(request: shredstream_shredstream_pb.SubscribePumpCreatesRequest, metadata?: grpc.Metadata, options?: Partial<grpc.CallOptions>): grpc.ClientReadableStream<shredstream_shredstream_pb.PumpCreate>;
    subscribeFills(request: shredstream_shredstream_pb.SubscribeFillsRequest, options?: Partial<grpc.CallOptions>): grpc.ClientReadableStream<shredstream_shredstream_pb.Fill>;
    subscribeFills(request: shredstream_shredstream_pb.SubscribeFillsRequest, metadata?: grpc.Metadata, options?: Partial<grpc.CallOptions>): grpc.ClientReadableStream<shredstream_shredstream_pb.Fill>;
}

export class ShredstreamProxyClient extends grpc.Client implements IShredstreamProxyClient {
    constructor(address: string, credentials: grpc.ChannelCredentials, options?: Partial<grpc.ClientOptions>);
    public subscribeEntries(request: shredstream_shredstream_pb.SubscribeEntriesRequest, options?: Partial<grpc.CallOptions>): grpc.ClientReadableStream<shredstream_shredstream_pb.Entry>;
    public subscribeEntries(request: shredstream_shredstream_pb.SubscribeEntriesRequest, metadata?: grpc.Metadata, options?: Partial<grpc.CallOptions>): grpc.ClientReadableStream<shredstream_shredstream_pb.Entry>;
    public subscribePumpCreates(request: shredstream_shredstream_pb.SubscribePumpCreatesRequest, options?: Partial<grpc.CallOptions>): grpc.ClientReadableStream<shredstream_shredstream_pb.PumpCreate>;
    public subscribePumpCreates(request: shredstream_shredstream_pb.SubscribePumpCreatesRequest, metadata?: grpc.Metadata, options?: Partial<grpc.CallOptions>): grpc.ClientReadableStream<shredstream_shredstream_pb.PumpCreate>;
    public subscribeFills(request: shredstream_shredstream_pb.SubscribeFillsRequest, options?: Partial<grpc.CallOptions>): grpc.ClientReadableStream<shredstream_shredstream_pb.Fill>;
    public subscribeFills(request: shredstream_shredstream_pb.SubscribeFillsRequest, metadata?: grpc.Metadata, options?: Partial<grpc.CallOptions>): grpc.ClientReadableStream<shredstream_shredstream_pb.Fill>;
}
