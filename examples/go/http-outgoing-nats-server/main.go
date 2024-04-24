// NOTE: This example is a work in-progress and will change significantly going forward

package main

import (
	"bufio"
	"context"
	"errors"
	"fmt"
	"io"
	"log"
	"math"
	"net/http"
	"os"
	"os/signal"
	"syscall"

	"github.com/nats-io/nats.go"
	wrpc "github.com/wrpc/wrpc/go"
	natswrpc "github.com/wrpc/wrpc/go/nats"
)

type Reader struct {
	ch     <-chan []byte
	buffer []byte
}

func (r *Reader) Read(p []byte) (int, error) {
	if len(r.buffer) > 0 {
		n := copy(p, r.buffer)
		r.buffer = r.buffer[n:]
		return n, nil
	}
	buf, ok := <-r.ch
	if !ok {
		return 0, io.EOF
	}
	n := copy(p, buf)
	r.buffer = buf[n:]
	return n, nil
}

type (
	_interface_wrpc__http__outgoing_handler struct{}
	_interface_wrpc__http__types            struct{}
)

var Wrpc struct {
	Http struct {
		OutgoingHandler _interface_wrpc__http__outgoing_handler
		Types           _interface_wrpc__http__types
	}
}

type _record_wrpc__http__types_request _interface_wrpc__http__types

func (_interface_wrpc__http__outgoing_handler) ServeHandle(c wrpc.Client, f func()) (func() error, error) {
	return c.Serve("wrpc:http/outgoing-handler@0.1.0", "handle", func(ctx context.Context, buf []byte, tx wrpc.Transmitter, inv wrpc.IncomingInvocation) error {
		paramCh := make(chan []byte, 128)
		paramStop, err := inv.SubscribeBody(func(ctx context.Context, buf []byte) {
			paramCh <- buf
		})
		if err != nil {
			return fmt.Errorf("failed to subscribe for `params`: %w", err)
		}
		defer func() {
			if err := paramStop(); err != nil {
				log.Printf("failed to unsubscribe from `params`: %s", err)
				return
			}
			close(paramCh)
		}()
		paramReader := bufio.NewReader(&Reader{
			buffer: buf,
			ch:     paramCh,
		})

		first := uint32(0)
		second := uint32(1)

		bodyCh := make(chan []byte, 128)
		bodyErrCh := make(chan error, 128)
		bodyStop, err := inv.SubscribeBodyElement([]*uint32{&first, &first}, bodyErrCh, func(ctx context.Context, path []uint32, buf []byte) {
			bodyCh <- buf
		})
		if err != nil {
			return fmt.Errorf("failed to subscribe for `body`: %w", err)
		}
		defer func() {
			if err := bodyStop(); err != nil {
				log.Printf("failed to unsubscribe from `body`: %s", err)
				return
			}
			close(bodyCh)
		}()
		//bodyReader := bufio.NewReader(&Reader{
		//	buffer: nil,
		//	ch:     bodyCh,
		//})

		trailerCh := make(chan []byte, 128)
		trailerErrCh := make(chan error, 128)
		trailerStop, err := inv.SubscribeBodyElement([]*uint32{&first, &second}, trailerErrCh, func(ctx context.Context, path []uint32, buf []byte) {
			trailerCh <- buf
		})
		if err != nil {
			return fmt.Errorf("failed to subscribe for `trailer`: %w", err)
		}
		defer func() {
			if err := trailerStop(); err != nil {
				log.Printf("failed to unsubscribe from `trailer`: %s", err)
				return
			}
			close(trailerCh)
		}()
		//trailerReader := bufio.NewReader(&Reader{
		//	buffer: nil,
		//	ch:     trailerCh,
		//})

		if err := inv.Accept(context.Background(), nil); err != nil {
			return fmt.Errorf("failed to complete handshake: %w", err)
		}

		status, err := paramReader.ReadByte()
		if err != nil {
			return fmt.Errorf("failed to read body status byte: %w", err)
		}

		body := []byte(nil)
		switch status {
		case 0:
			// pending
		case 1:
			n, err := wrpc.ReadUint32(paramReader)
			if err != nil {
				return fmt.Errorf("failed to read body length: %w", err)
			}
			body = make([]byte, int(n))
			rn, err := paramReader.Read(body)
			if err != nil {
				return fmt.Errorf("failed to read body: %w", err)
			}
			if rn != int(n) {
				return fmt.Errorf("invalid amount of body bytes read, expected %d, got %d", n, rn)
			}
		default:
			return errors.New("invalid status byte")
		}

		status, err = paramReader.ReadByte()
		if err != nil {
			return fmt.Errorf("failed to read trailer status byte: %w", err)
		}
		switch status {
		case 0:
			// pending
		case 1:
			n, err := wrpc.ReadUint32(paramReader)
			if err != nil {
				return fmt.Errorf("failed to read trailer length: %w", err)
			}
			if n > 0 {
				return errors.New("trailers not supported yet")
			}
		default:
			return errors.New("invalid status byte")
		}

		disc, err := wrpc.ReadUint32(paramReader)
		if err != nil {
			return fmt.Errorf("failed to read method discriminant: %w", err)
		}
		switch disc {
		case 0:
		// GET
		default:
			return errors.New("only GET currently supported")
		}

		pathWithQuery := ""
		status, err = paramReader.ReadByte()
		if err != nil {
			return fmt.Errorf("failed to read `path_with_query` status byte: %w", err)
		}
		switch status {
		case 0:
		case 1:
			pathWithQuery, err = wrpc.ReadString(paramReader)
			if err != nil {
				return fmt.Errorf("failed to read path_with_query: %w", err)
			}
		default:
			return errors.New("invalid status byte")
		}

		scheme := ""
		status, err = paramReader.ReadByte()
		if err != nil {
			return fmt.Errorf("failed to read `scheme` status byte: %w", err)
		}
		switch status {
		case 0:
		case 1:
			disc, err := wrpc.ReadUint32(paramReader)
			if err != nil {
				return fmt.Errorf("failed to read scheme discriminant: %w", err)
			}
			switch disc {
			case 0:
				scheme = "http"
			case 1:
				scheme = "https"
			case 2:
				scheme, err = wrpc.ReadString(paramReader)
				if err != nil {
					return fmt.Errorf("failed to read scheme: %w", err)
				}
			default:
				return errors.New("invalid scheme")
			}
		default:
			return errors.New("invalid status byte")
		}

		authority := ""
		status, err = paramReader.ReadByte()
		if err != nil {
			return fmt.Errorf("failed to read `authority` status byte: %w", err)
		}
		switch status {
		case 0:
		case 1:
			authority, err = wrpc.ReadString(paramReader)
			if err != nil {
				return fmt.Errorf("failed to read authority: %w", err)
			}
		default:
			return errors.New("invalid status byte")
		}

		n, err := wrpc.ReadUint32(paramReader)
		if err != nil {
			return fmt.Errorf("failed to read header length: %w", err)
		}
		if n > 0 {
			return errors.New("headers not supported yet")
		}

		resp, err := http.Get(fmt.Sprintf("%s://%s/%s", scheme, authority, pathWithQuery))
		if err != nil {
			return fmt.Errorf("request failed: %w", err)
		}

		respBody, err := io.ReadAll(resp.Body)
		if err != nil {
			return fmt.Errorf("failed to read body: %w", err)
		}
		res := []byte{0, 1}
		res = wrpc.AppendUint32(res, uint32(len(respBody)))
		res = append(res, respBody...)
		res = append(res, 1, 0) // empty trailers
		res = wrpc.AppendUint16(res, uint16(resp.StatusCode))

		nh := len(resp.Header)
		if nh > math.MaxUint32 {
			return fmt.Errorf("header count %d overflows uint32", nh)
		}
		res = wrpc.AppendUint32(res, uint32(nh))
		for header, values := range resp.Header {
			nv := len(values)
			if nv > math.MaxUint32 {
				return fmt.Errorf("header value count %d overflows uint32", nv)
			}
			res, err = wrpc.AppendString(res, header)
			if err != nil {
				return fmt.Errorf("failed to encode header name: %w", err)
			}
			res = wrpc.AppendUint32(res, uint32(nv))
			for _, value := range values {
				res, err = wrpc.AppendString(res, value)
				if err != nil {
					return fmt.Errorf("failed to encode header value: %w", err)
				}
			}
		}
		if err := tx.Transmit(context.Background(), nil, res); err != nil {
			return fmt.Errorf("failed to transmit result: %w", err)
		}

		return nil
	})
}

func run() error {
	nc, err := nats.Connect(nats.DefaultURL)
	if err != nil {
		return fmt.Errorf("failed to connect to NATS.io: %w", err)
	}
	defer nc.Close()

	stop, err := Wrpc.Http.OutgoingHandler.ServeHandle(natswrpc.NewClient(nc, "go"), func() {})
	if err != nil {
		return err
	}

	signalCh := make(chan os.Signal, 1)
	signal.Notify(signalCh, syscall.SIGINT)
	<-signalCh
	if err = stop(); err != nil {
		log.Printf("failed to stop serving: %s", err)
	}
	if err = nc.Drain(); err != nil {
		log.Printf("failed to drain NATS.io connection: %s", err)
	}
	return nil
}

func init() {
	log.SetFlags(0)
	log.SetOutput(os.Stderr)
}

func main() {
	if err := run(); err != nil {
		log.Fatal(err)
	}
}
