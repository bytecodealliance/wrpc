package wrpc_test

import (
	"bytes"
	"context"
	"errors"
	"io"
	"net"
	"slices"
	"testing"
	"time"

	wrpc "wrpc.io/go"
)

type echoParams = wrpc.Tuple2[<-chan []byte, <-chan string]

var (
	echoParamsCodec  = wrpc.Tuple2Codec[<-chan []byte, <-chan string]{wrpc.ByteStreamCodec{}, wrpc.FutureCodec[string]{wrpc.StringCodec{}}}
	echoResultsCodec = wrpc.Tuple1Codec[<-chan []byte]{wrpc.ByteStreamCodec{}}
	futsParamsCodec  = wrpc.Tuple1Codec[<-chan []<-chan string]{wrpc.StreamCodec[<-chan string]{wrpc.FutureCodec[string]{wrpc.StringCodec{}}}}
	futsResultsCodec = wrpc.Tuple1Codec[[]string]{wrpc.ListCodec[string]{wrpc.StringCodec{}}}
	unitCodec        = wrpc.UnitCodec{}
)

func serveTestConn(ctx context.Context, conn net.Conn) error {
	lis := wrpc.NewListener(conn, conn)
	for {
		h, c, err := lis.Accept(ctx)
		if err == io.EOF {
			return nil
		}
		if err != nil {
			return err
		}
		switch h.Name {
		case "echo":
			err = wrpc.Serve(ctx, c, echoParamsCodec, echoResultsCodec, func(ctx context.Context, p echoParams) (wrpc.Tuple1[<-chan []byte], error) {
				out := make(chan []byte)
				go func() {
					defer close(out)
					for b := range p.V0 {
						out <- b
					}
					out <- []byte(<-p.V1)
				}()
				return wrpc.Tuple1[<-chan []byte]{out}, nil
			})
		case "futs":
			err = wrpc.Serve(ctx, c, futsParamsCodec, futsResultsCodec, func(ctx context.Context, p wrpc.Tuple1[<-chan []<-chan string]) (wrpc.Tuple1[[]string], error) {
				var fs []<-chan string
				for chunk := range p.V0 {
					fs = append(fs, chunk...)
				}
				var vs []string
				for _, f := range fs {
					vs = append(vs, <-f)
				}
				return wrpc.Tuple1[[]string]{vs}, nil
			})
		default:
			err = wrpc.Serve(ctx, c, unitCodec, unitCodec, func(context.Context, struct{}) (struct{}, error) {
				return struct{}{}, errors.New("handler failed")
			})
		}
		if err != nil {
			return err
		}
	}
}

func TestInvokeServe(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer l.Close()
	srvErr := make(chan error, 1)
	go func() {
		conn, err := l.Accept()
		if err != nil {
			srvErr <- err
			return
		}
		defer conn.Close()
		srvErr <- serveTestConn(ctx, conn)
	}()

	conn, err := net.Dial("tcp", l.Addr().String())
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	c := wrpc.NewFramed(conn, conn)

	for range 2 {
		s := make(chan []byte)
		f := make(chan string, 1)
		go func() {
			s <- []byte("foo")
			s <- []byte("bar")
			close(s)
			f <- "baz"
			close(f)
		}()
		res, inv, err := wrpc.Invoke(ctx, c, "test", "echo", echoParamsCodec, echoResultsCodec, echoParams{s, f})
		if err != nil {
			t.Fatal(err)
		}
		done := make(chan error, 1)
		go func() { done <- inv.Wait() }()
		var buf bytes.Buffer
		for b := range res.V0 {
			buf.Write(b)
		}
		if err := <-done; err != nil {
			t.Fatal(err)
		}
		if buf.String() != "foobarbaz" {
			t.Fatalf("unexpected result `%s`", buf.String())
		}
	}

	s := make(chan []<-chan string)
	go func() {
		a, b, c := make(chan string, 1), make(chan string, 1), make(chan string, 1)
		a <- "a"
		close(a)
		s <- []<-chan string{a, b}
		s <- []<-chan string{c}
		close(s)
		b <- "b"
		close(b)
		c <- "c"
		close(c)
	}()
	res, inv, err := wrpc.Invoke(ctx, c, "test", "futs", futsParamsCodec, futsResultsCodec, wrpc.Tuple1[<-chan []<-chan string]{s})
	if err != nil {
		t.Fatal(err)
	}
	if err := inv.Wait(); err != nil {
		t.Fatal(err)
	}
	if !slices.Equal(res.V0, []string{"a", "b", "c"}) {
		t.Fatalf("unexpected result `%v`", res.V0)
	}

	_, inv, err = wrpc.Invoke(ctx, c, "test", "fail", unitCodec, unitCodec, struct{}{})
	if err == nil {
		err = inv.Wait()
	}
	if err == nil {
		t.Fatal("expected a failed handler to fail the invocation")
	}
	var serr *wrpc.ServeError
	if err := <-srvErr; !errors.As(err, &serr) || serr.Fault != wrpc.FaultHandler {
		t.Fatalf("unexpected serve error: %v", err)
	}
}
