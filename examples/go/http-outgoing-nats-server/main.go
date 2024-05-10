package main

import (
	"bufio"
	"context"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"os"
	"os/signal"
	"syscall"

	"github.com/nats-io/nats.go"
	wrpc "github.com/wrpc/wrpc/go"
	"github.com/wrpc/wrpc/go/interface/http/outgoing_handler"
	"github.com/wrpc/wrpc/go/interface/http/types"
	wrpcnats "github.com/wrpc/wrpc/go/nats"
)

type incomingBody struct {
	body      io.Reader
	trailer   http.Header
	trailerRx wrpc.Receiver[[]*wrpc.Tuple2[string, [][]byte]]
}

func (r *incomingBody) Read(b []byte) (int, error) {
	n, err := r.body.Read(b)
	if err == io.EOF {
		trailers, err := r.trailerRx.Receive()
		if err != nil {
			return 0, fmt.Errorf("failed to receive trailers: %w", err)
		}
		for _, header := range trailers {
			for _, value := range header.V1 {
				r.trailer.Add(header.V0, string(value))
			}
		}
		return n, io.EOF
	}
	return n, err
}

type outgoingBody struct {
	body      io.ReadCloser
	trailer   http.Header
	trailerCh chan<- []*wrpc.Tuple2[string, [][]byte]
}

func (r *outgoingBody) Read(b []byte) (int, error) {
	slog.Debug("reading HTTP body chunk", "len", len(b))
	n, err := r.body.Read(b)
	slog.Debug("read HTTP body chunk", "len", n)
	if err == io.EOF {
		slog.Debug("HTTP body reached EOF, reading trailers")
		trailers := make([]*wrpc.Tuple2[string, [][]byte], 0, len(r.trailer))
		for header, values := range r.trailer {
			bs := make([][]byte, len(values))
			for i, value := range values {
				bs[i] = []byte(value)
			}
			trailers = append(trailers, &wrpc.Tuple2[string, [][]byte]{V0: header, V1: bs})
		}
		slog.Debug("sending trailers")
		r.trailerCh <- trailers
		close(r.trailerCh)
		return n, io.EOF
	}
	return n, err
}

func (r *outgoingBody) Close() error {
	return r.body.Close()
}

type trailerReceiver <-chan []*wrpc.Tuple2[string, [][]byte]

func (r trailerReceiver) Receive() ([]*wrpc.Tuple2[string, [][]byte], error) {
	trailers, ok := <-r
	if !ok {
		return nil, errors.New("trailer receiver channel closed")
	}
	return trailers, nil
}

func (r trailerReceiver) Ready() bool {
	return false
}

func run() error {
	nc, err := nats.Connect(nats.DefaultURL)
	if err != nil {
		return fmt.Errorf("failed to connect to NATS.io: %w", err)
	}
	defer nc.Close()

	stop, err := outgoing_handler.ServeHandle(wrpcnats.NewClient(nc, "go"), func(ctx context.Context, request *types.RequestRecord, opts *types.RequestOptionsRecord) (*wrpc.Result[types.ResponseRecord, types.ErrorCodeVariant], error) {
		var method string
		switch disc := request.Method.Discriminant(); disc {
		case types.MethodDiscriminant_Get:
			method = "GET"
		case types.MethodDiscriminant_Head:
			method = "HEAD"
		case types.MethodDiscriminant_Post:
			method = "POST"
		case types.MethodDiscriminant_Put:
			method = "PUT"
		case types.MethodDiscriminant_Delete:
			method = "DELETE"
		case types.MethodDiscriminant_Connect:
			method = "CONNECT"
		case types.MethodDiscriminant_Options:
			method = "OPTIONS"
		case types.MethodDiscriminant_Trace:
			method = "TRACE"
		case types.MethodDiscriminant_Patch:
			method = "PATCH"
		case types.MethodDiscriminant_Other:
			var ok bool
			method, ok = request.Method.GetOther()
			if !ok {
				return nil, errors.New("invalid HTTP method")
			}
		default:
			return nil, fmt.Errorf("invalid HTTP method discriminant %v", disc)
		}
		scheme := "https"
		if request.Scheme != nil {
			switch disc := request.Scheme.Discriminant(); disc {
			case types.SchemeDiscriminant_Http:
				scheme = "http"
			case types.SchemeDiscriminant_Https:
				scheme = "https"
			case types.SchemeDiscriminant_Other:
				var ok bool
				scheme, ok = request.Scheme.GetOther()
				if !ok {
					return nil, errors.New("invalid scheme")
				}
			default:
				return nil, fmt.Errorf("invalid scheme discriminant %v", disc)
			}
		}
		url := fmt.Sprintf("%s://", scheme)
		if request.Authority != nil {
			url = fmt.Sprintf("%s%s", url, *request.Authority)
		}
		if request.PathWithQuery != nil {
			url = fmt.Sprintf("%s%s", url, *request.PathWithQuery)
		}

		var trailer http.Header
		req, err := http.NewRequest(method, url, &incomingBody{body: request.Body, trailer: trailer, trailerRx: request.Trailers})
		if err != nil {
			return nil, fmt.Errorf("failed to construct a new HTTP request: %w", err)
		}
		req.Trailer = trailer
		for _, header := range request.Headers {
			for _, value := range header.V1 {
				slog.DebugContext(ctx, "adding header", "name", header, "value", string(value))
				req.Header.Add(header.V0, string(value))
			}
		}
		slog.DebugContext(ctx, "sending HTTP request", "url", url, "method", method)
		resp, err := http.DefaultClient.Do(req)
		if err != nil {
			return nil, fmt.Errorf("request failed: %w", err)
		}

		headers := make([]*wrpc.Tuple2[string, [][]byte], 0, len(resp.Header))
		for header, values := range resp.Header {
			bs := make([][]byte, len(values))
			for i, value := range values {
				bs[i] = []byte(value)
			}
			headers = append(headers, &wrpc.Tuple2[string, [][]byte]{V0: header, V1: bs})
		}

		trailerCh := make(chan []*wrpc.Tuple2[string, [][]byte], 1)
		body := &outgoingBody{body: resp.Body, trailer: resp.Trailer, trailerCh: trailerCh}
		return &wrpc.Result[types.ResponseRecord, types.ErrorCodeVariant]{
			Ok: &types.ResponseRecord{
				Body:     wrpc.NewPendingByteReader(bufio.NewReader(body)),
				Trailers: trailerReceiver(trailerCh),
				Status:   uint16(resp.StatusCode),
				Headers:  headers,
			},
		}, nil
	})
	if err != nil {
		return err
	}

	signalCh := make(chan os.Signal, 1)
	signal.Notify(signalCh, syscall.SIGINT)
	<-signalCh

	if err = stop(); err != nil {
		return fmt.Errorf("failed to stop serving: %w", err)
	}
	if err = nc.Drain(); err != nil {
		return fmt.Errorf("failed to drain NATS.io connection: %w", err)
	}
	return nil
}

func init() {
	slog.SetDefault(slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{
		Level: slog.LevelDebug, ReplaceAttr: func(groups []string, a slog.Attr) slog.Attr {
			if a.Key == slog.TimeKey {
				return slog.Attr{}
			}
			return a
		},
	})))
}

func main() {
	if err := run(); err != nil {
		slog.Error("failed to run", "err", err)
		os.Exit(1)
	}
}
