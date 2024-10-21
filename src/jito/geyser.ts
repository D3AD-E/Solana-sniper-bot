import * as grpc from '@grpc/grpc-js';
import * as protoLoader from '@grpc/proto-loader';
// Load the compiled gRPC services
const PROTO_PATH = './proto/geyser.proto'; // Path to your compiled .proto file

// Load gRPC package definition
const packageDefinition = protoLoader.loadSync(PROTO_PATH, {
  keepCase: true,
  longs: String,
  enums: String,
  defaults: true,
  oneofs: true,
});

// Load the gRPC package definition
const protoDescriptor = grpc.loadPackageDefinition(packageDefinition) as any;

// Get the Geyser service
const GeyserService = protoDescriptor.solana.geyser.Geyser;

// Create a gRPC client
const client = new GeyserService(
  '127.0.0.1:10000', // Replace with the actual gRPC server address and port
  grpc.credentials.createInsecure(), // Use createSsl() for secure connections
);

export default client;
