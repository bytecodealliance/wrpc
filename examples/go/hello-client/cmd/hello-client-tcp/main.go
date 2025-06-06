package main

import (
	"log"

	app "wrpc.io/examples/go/hello-client"
	wrpctcp "wrpc.io/go/x/tcp"
)

func run() error {
	addr := "[::1]:7761"
	return app.Run(addr, wrpctcp.NewInvoker(addr))
}

func main() {
	if err := run(); err != nil {
		log.Fatal(err)
	}
}
