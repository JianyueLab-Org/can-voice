package main

import (
	"crypto/tls"
	"flag"
	"log"
)

func main() {
	addr := flag.String("addr", ":64739", "UDP listen address")
	certFile := flag.String("cert", "", "TLS certificate chain (PEM); self-signed if empty")
	keyFile := flag.String("key", "", "TLS private key (PEM)")
	flag.Parse()

	var tlsConf tls.Config
	if *certFile != "" {
		cert, err := tls.LoadX509KeyPair(*certFile, *keyFile)
		if err != nil {
			log.Fatalf("load certificate: %v", err)
		}
		tlsConf.Certificates = []tls.Certificate{cert}
	} else {
		cert, err := SelfSignedCert()
		if err != nil {
			log.Fatalf("generate self-signed certificate: %v", err)
		}
		tlsConf.Certificates = []tls.Certificate{cert}
		log.Print("using a self-signed certificate; clients must pass -insecure")
	}

	ln, err := Listen(*addr, &tlsConf)
	if err != nil {
		log.Fatalf("listen on %s: %v", *addr, err)
	}
	log.Printf("probe server listening on %s, alpn=%s", *addr, ALPN)
	Serve(ln)
}
