// GENERATED CODE -- DO NOT EDIT!

'use strict';
var grpc = require('@grpc/grpc-js');
var shredstream_shredstream_pb = require('../shredstream/shredstream_pb.js');
var google_protobuf_timestamp_pb = require('google-protobuf/google/protobuf/timestamp_pb.js');

function serialize_shredstream_Entry(arg) {
  if (!(arg instanceof shredstream_shredstream_pb.Entry)) {
    throw new Error('Expected argument of type shredstream.Entry');
  }
  return Buffer.from(arg.serializeBinary());
}

function deserialize_shredstream_Entry(buffer_arg) {
  return shredstream_shredstream_pb.Entry.deserializeBinary(new Uint8Array(buffer_arg));
}

function serialize_shredstream_Fill(arg) {
  if (!(arg instanceof shredstream_shredstream_pb.Fill)) {
    throw new Error('Expected argument of type shredstream.Fill');
  }
  return Buffer.from(arg.serializeBinary());
}

function deserialize_shredstream_Fill(buffer_arg) {
  return shredstream_shredstream_pb.Fill.deserializeBinary(new Uint8Array(buffer_arg));
}

function serialize_shredstream_PumpCreate(arg) {
  if (!(arg instanceof shredstream_shredstream_pb.PumpCreate)) {
    throw new Error('Expected argument of type shredstream.PumpCreate');
  }
  return Buffer.from(arg.serializeBinary());
}

function deserialize_shredstream_PumpCreate(buffer_arg) {
  return shredstream_shredstream_pb.PumpCreate.deserializeBinary(new Uint8Array(buffer_arg));
}

function serialize_shredstream_SubscribeEntriesRequest(arg) {
  if (!(arg instanceof shredstream_shredstream_pb.SubscribeEntriesRequest)) {
    throw new Error('Expected argument of type shredstream.SubscribeEntriesRequest');
  }
  return Buffer.from(arg.serializeBinary());
}

function deserialize_shredstream_SubscribeEntriesRequest(buffer_arg) {
  return shredstream_shredstream_pb.SubscribeEntriesRequest.deserializeBinary(new Uint8Array(buffer_arg));
}

function serialize_shredstream_SubscribeFillsRequest(arg) {
  if (!(arg instanceof shredstream_shredstream_pb.SubscribeFillsRequest)) {
    throw new Error('Expected argument of type shredstream.SubscribeFillsRequest');
  }
  return Buffer.from(arg.serializeBinary());
}

function deserialize_shredstream_SubscribeFillsRequest(buffer_arg) {
  return shredstream_shredstream_pb.SubscribeFillsRequest.deserializeBinary(new Uint8Array(buffer_arg));
}

function serialize_shredstream_SubscribePumpCreatesRequest(arg) {
  if (!(arg instanceof shredstream_shredstream_pb.SubscribePumpCreatesRequest)) {
    throw new Error('Expected argument of type shredstream.SubscribePumpCreatesRequest');
  }
  return Buffer.from(arg.serializeBinary());
}

function deserialize_shredstream_SubscribePumpCreatesRequest(buffer_arg) {
  return shredstream_shredstream_pb.SubscribePumpCreatesRequest.deserializeBinary(new Uint8Array(buffer_arg));
}


// Shredstream Proxy
//
var ShredstreamProxyService = exports.ShredstreamProxyService = {
  subscribeEntries: {
    path: '/shredstream.ShredstreamProxy/SubscribeEntries',
    requestStream: false,
    responseStream: true,
    requestType: shredstream_shredstream_pb.SubscribeEntriesRequest,
    responseType: shredstream_shredstream_pb.Entry,
    requestSerialize: serialize_shredstream_SubscribeEntriesRequest,
    requestDeserialize: deserialize_shredstream_SubscribeEntriesRequest,
    responseSerialize: serialize_shredstream_Entry,
    responseDeserialize: deserialize_shredstream_Entry,
  },
  // Pre-parsed pump.fun launches: no transaction decoding on the consumer side.
subscribePumpCreates: {
    path: '/shredstream.ShredstreamProxy/SubscribePumpCreates',
    requestStream: false,
    responseStream: true,
    requestType: shredstream_shredstream_pb.SubscribePumpCreatesRequest,
    responseType: shredstream_shredstream_pb.PumpCreate,
    requestSerialize: serialize_shredstream_SubscribePumpCreatesRequest,
    requestDeserialize: deserialize_shredstream_SubscribePumpCreatesRequest,
    responseSerialize: serialize_shredstream_PumpCreate,
    responseDeserialize: deserialize_shredstream_PumpCreate,
  },
  // Buys the sniper fired, for the process that handles selling.
subscribeFills: {
    path: '/shredstream.ShredstreamProxy/SubscribeFills',
    requestStream: false,
    responseStream: true,
    requestType: shredstream_shredstream_pb.SubscribeFillsRequest,
    responseType: shredstream_shredstream_pb.Fill,
    requestSerialize: serialize_shredstream_SubscribeFillsRequest,
    requestDeserialize: deserialize_shredstream_SubscribeFillsRequest,
    responseSerialize: serialize_shredstream_Fill,
    responseDeserialize: deserialize_shredstream_Fill,
  },
};

exports.ShredstreamProxyClient = grpc.makeGenericClientConstructor(ShredstreamProxyService, 'ShredstreamProxy');
