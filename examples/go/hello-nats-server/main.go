package main

import (
	"fmt"
	"log"
	"math"
	"os"

	"github.com/nats-io/nats.go"
	"github.com/tetratelabs/wabin/leb128"
)

func encode_string(s string) ([]byte, error) {
	n := len(s)
	if n > math.MaxUint32 {
		return nil, fmt.Errorf("response UTF-8 string byte length of %d overflows u32", n)
	}
	return append(leb128.EncodeUint32(uint32(n)), s...), nil
}

func handle_hello(nc *nats.Conn, results string) (err error) {
	b, err := encode_string("hello from Go")
	if err != nil {
		return fmt.Errorf("failed to encode `hello`: %w", err)
	}
	n := len(b)
	maxPayload := nc.MaxPayload()
	if int64(n) > maxPayload {
		return fmt.Errorf("response length of %d exceeds NATS.io max payload of %d", n, maxPayload)
	}
	if err := nc.Publish(results, b); err != nil {
		return fmt.Errorf("failed to publish response on result subject `%s`: %w", results, err)
	}
	return nil
}

func run() error {
	// Connect to a server
	nc, err := nats.Connect(nats.DefaultURL)
	if err != nil {
		return fmt.Errorf("failed to connect to NATS.io: %w", err)
	}
	defer func() {
		if err := nc.Drain(); err != nil {
			log.Printf("failed to drain NATS.io connection: %s", err)
		}
	}()
	defer nc.Close()

	hello := make(chan *nats.Msg)
	helloSub, err := nc.ChanSubscribe("go.wrpc.0.0.1.wrpc-examples:hello/handler.hello", hello)
	if err != nil {
		return fmt.Errorf("failed to subscribe to `hello` invocations: %w", err)
	}
	defer func() {
		if err := helloSub.Unsubscribe(); err != nil {
			log.Printf("failed to unsubscribe from `hello`: %s", err)
			return
		}
		close(hello)
	}()
	for msg := range hello {
		if err := msg.Respond([]byte{}); err != nil {
			log.Printf("failed to complete handshake on `%s` subject", msg.Reply)
			continue
		}

		if err := handle_hello(nc, fmt.Sprintf("%s.results", msg.Reply)); err != nil {
			log.Printf("failed to handle `hello`: %s", err)
			b, err := encode_string(fmt.Sprintf("%s", err))
			if err != nil {
				log.Printf("failed to encode `hello` handling error: %s", err)
				// Encoding the error failed, let's try encoding the encoding error, shall we?
				b, err = encode_string(fmt.Sprintf("failed to encode error: %s", err))
				if err != nil {
					log.Printf("failed to encode `hello` handling error encoding error: %s", err)
					// Well, we're out of luck at this point, let's just send an empty string
					b = []byte{0}
				}
			}
			if len(b) > int(nc.MaxPayload()) {
				// TODO: split the payload into multiple chunks
				b = []byte{0}
			}
			if err = nc.Publish(fmt.Sprintf("%s.error", msg.Reply), b); err != nil {
				log.Printf("failed to send error to client: %s", err)
			}
		}
	}
	return nil
}

func main() {
	log.SetFlags(0)
	log.SetOutput(os.Stderr)

	if err := run(); err != nil {
		log.Fatal(err)
	}
}
