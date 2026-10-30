package wrpc_test

import (
	"bytes"
	"context"
	"io"
	"strings"
	"testing"

	wrpc "wrpc.io/go"
)

type nopWriteCloser struct{ io.Writer }

func (nopWriteCloser) Close() error { return nil }

func TestFramedServerHeaderNameLimit(t *testing.T) {
	instance := strings.Repeat("a", 2000)
	for _, max := range []uint32{0, 2000} {
		var opts []wrpc.FramedServerOption
		if max > 0 {
			opts = append(opts, wrpc.WithMaxHeaderNameLen(max, 1))
		}
		srv := wrpc.NewFramedServer(context.Background(), opts...)
		stop, err := srv.Serve(instance, "f", func(context.Context, wrpc.IndexWriteCloser, wrpc.IndexReadCloser) {})
		if err != nil {
			t.Fatal(err)
		}
		var hdr bytes.Buffer
		hdr.WriteByte(0)
		if _, err := wrpc.WriteString(instance, &hdr); err != nil {
			t.Fatal(err)
		}
		if _, err := wrpc.WriteString("f", &hdr); err != nil {
			t.Fatal(err)
		}
		err = srv.Accept(context.Background(), nopWriteCloser{io.Discard}, io.NopCloser(&hdr))
		if (err == nil) != (max > 0) {
			t.Fatalf("unexpected accept result with maximum %d: %v", max, err)
		}
		if err := stop(); err != nil {
			t.Fatal(err)
		}
	}
}
