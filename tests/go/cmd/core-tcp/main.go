package main

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"log"
	"net"
	"os"
	"slices"

	wrpc "wrpc.io/go"
)

type echoParams = wrpc.Tuple2[<-chan []byte, <-chan string]

type futsParams = wrpc.Tuple1[<-chan []<-chan string]

var (
	echoParamsCodec  = wrpc.Tuple2Codec[<-chan []byte, <-chan string]{C0: wrpc.ByteStreamCodec{}, C1: wrpc.FutureCodec[string]{Elem: wrpc.StringCodec{}}}
	echoResultsCodec = wrpc.Tuple1Codec[<-chan []byte]{C0: wrpc.ByteStreamCodec{}}
	futsParamsCodec  = wrpc.Tuple1Codec[<-chan []<-chan string]{C0: wrpc.StreamCodec[<-chan string]{Elem: wrpc.FutureCodec[string]{Elem: wrpc.StringCodec{}}}}
	futsResultsCodec = wrpc.Tuple1Codec[[]string]{C0: wrpc.ListCodec[string]{Elem: wrpc.StringCodec{}}}
)

func echo(ctx context.Context, p echoParams) (wrpc.Tuple1[<-chan []byte], error) {
	out := make(chan []byte)
	go func() {
		defer close(out)
		for b := range p.V0 {
			out <- b
		}
		out <- []byte(<-p.V1)
	}()
	return wrpc.Tuple1[<-chan []byte]{V0: out}, nil
}

func futs(ctx context.Context, p futsParams) (wrpc.Tuple1[[]string], error) {
	var fs []<-chan string
	for chunk := range p.V0 {
		fs = append(fs, chunk...)
	}
	var vs []string
	for _, f := range fs {
		vs = append(vs, <-f)
	}
	return wrpc.Tuple1[[]string]{V0: vs}, nil
}

func serve(ctx context.Context) error {
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return err
	}
	defer l.Close()
	fmt.Println(l.Addr())
	conn, err := l.Accept()
	if err != nil {
		return err
	}
	defer conn.Close()
	lis := wrpc.NewListener(conn, conn)
	for {
		h, c, err := lis.Accept(ctx)
		if err == io.EOF {
			return nil
		}
		if err != nil {
			return err
		}
		switch {
		case h.Instance == "test" && h.Name == "echo":
			err = wrpc.Serve(ctx, c, echoParamsCodec, echoResultsCodec, echo)
		case h.Instance == "test" && h.Name == "futs":
			err = wrpc.Serve(ctx, c, futsParamsCodec, futsResultsCodec, futs)
		default:
			err = fmt.Errorf("unknown function `%s.%s`", h.Instance, h.Name)
		}
		if err != nil {
			return err
		}
	}
}

func client(ctx context.Context, addr string) error {
	conn, err := net.Dial("tcp", addr)
	if err != nil {
		return err
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
		res, inv, err := wrpc.Invoke(ctx, c, "test", "echo", echoParamsCodec, echoResultsCodec, echoParams{V0: s, V1: f})
		if err != nil {
			return err
		}
		done := make(chan error, 1)
		go func() { done <- inv.Wait() }()
		var buf bytes.Buffer
		for b := range res.V0 {
			buf.Write(b)
		}
		if err := <-done; err != nil {
			return err
		}
		if buf.String() != "foobarbaz" {
			return fmt.Errorf("unexpected echo result `%s`", buf.String())
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
	res, inv, err := wrpc.Invoke(ctx, c, "test", "futs", futsParamsCodec, futsResultsCodec, futsParams{V0: s})
	if err != nil {
		return err
	}
	if err := inv.Wait(); err != nil {
		return err
	}
	if !slices.Equal(res.V0, []string{"a", "b", "c"}) {
		return fmt.Errorf("unexpected futs result `%v`", res.V0)
	}
	return nil
}

func main() {
	ctx := context.Background()
	var err error
	switch {
	case len(os.Args) == 2 && os.Args[1] == "server":
		err = serve(ctx)
	case len(os.Args) == 3 && os.Args[1] == "client":
		err = client(ctx, os.Args[2])
	default:
		err = errors.New("usage: core-tcp server | core-tcp client ADDR")
	}
	if err != nil {
		log.Fatal(err)
	}
}
