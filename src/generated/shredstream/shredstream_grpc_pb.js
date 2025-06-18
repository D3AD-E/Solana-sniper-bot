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

function serialize_shredstream_SubscribeEntriesRequest(arg) {
  if (!(arg instanceof shredstream_shredstream_pb.SubscribeEntriesRequest)) {
    throw new Error('Expected argument of type shredstream.SubscribeEntriesRequest');
  }
  return Buffer.from(arg.serializeBinary());
}

function deserialize_shredstream_SubscribeEntriesRequest(buffer_arg) {
  return shredstream_shredstream_pb.SubscribeEntriesRequest.deserializeBinary(new Uint8Array(buffer_arg));
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
};

exports.ShredstreamProxyClient = grpc.makeGenericClientConstructor(ShredstreamProxyService, 'ShredstreamProxy');
