package main

import (
	"fmt"
	"log"
	"net"

	app "wrpc.io/examples/go/hello-client"
	wrpc "wrpc.io/go"
)

func run() error {
	addr := "[::1]:7761"
	conn, err := net.Dial("tcp", addr)
	if err != nil {
		return fmt.Errorf("failed to connect to `%s`: %w", addr, err)
	}
	defer conn.Close()
	return app.Run(addr, wrpc.NewFramed(conn, conn))
}

func main() {
	if err := run(); err != nil {
		log.Fatal(err)
	}
}
