const {Transport} = require('./index.js')

/** @type {import('./index.js').TransportConfig} */
const config = {
  keypair: Buffer.from('08021220fa105479b9c18626c6d302897abd09983915d94de351fa8ee08a5556665e81df', 'hex'),
  // defaults taken from rust-libp2p
  handshakeTimeout: 5_000,
  maxIdleTimeout: 30_000,
  keepAliveInterval: 15_000,
  maxConcurrentStreamLimit: 256,
  maxStreamData: 10_000_000,
  maxConnectionData: 15_000_000,
}

const transport = Transport.new(config)
console.log(transport)

const listener = transport.createListener('0.0.0.0:3000')
console.log(listener)

transport.dial("/ip4/127.0.0.1/udp/3000/quic-v1").then(c => console.log(c), console.log)