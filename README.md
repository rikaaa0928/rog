srcAddr -(http/socks5)-> connAddr(client) -(grpc)-> listenAddr(proxy) -(grpc)-> listenAddr(server) -(tcp/udp)-> dstAddr


listen --> { accept(auth) --> { dial --> handle stream } }