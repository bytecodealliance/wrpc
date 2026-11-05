package wrpc_test

import (
	"bytes"
	"context"
	"testing"

	wrpc "wrpc.io/go"
)

func TestWriteFutureUnit(t *testing.T) {
	ch := make(chan struct{}, 1)
	ch <- struct{}{}
	close(ch)
	var buf bytes.Buffer
	if write, err := wrpc.WriteFutureUnit(ch, &buf); err != nil || write != nil || !bytes.Equal(buf.Bytes(), []byte{1}) {
		t.Fatalf("resolved future should be written as ready, got `%v`, `%v`", buf.Bytes(), err)
	}
	if _, err := wrpc.WriteFutureUnit(nil, &buf); err == nil {
		t.Fatal("writing a nil future should fail")
	}
}

func TestWriteFutureNested(t *testing.T) {
	inner := make(chan byte, 1)
	inner <- 'x'
	outer := make(chan (<-chan byte), 1)
	outer <- inner
	var buf bytes.Buffer
	write, err := wrpc.WriteFuture(outer, &buf, func(v <-chan byte, w wrpc.ByteWriter) (func(context.Context, wrpc.IndexWriter) error, error) {
		return wrpc.WriteFuture(v, w, func(v byte, w wrpc.ByteWriter) (func(context.Context, wrpc.IndexWriter) error, error) {
			return nil, w.WriteByte(v)
		})
	})
	if err != nil || !bytes.Equal(buf.Bytes(), []byte{0}) {
		t.Fatalf("future should be written as pending, got `%v`, `%v`", buf.Bytes(), err)
	}
	var dst sink
	w := wrpc.NewFrameStreamWriter(context.Background(), &dst)
	if err := wrpc.IndexWrite(write, 0)(context.Background(), w); err != nil {
		t.Fatalf("failed to write pending future: %s", err)
	}
	assertClose(t, w)

	r := bytes.NewBuffer(dst.Bytes())
	assertReadFrame(t, r, []byte{0}, 0)
	assertReadFrame(t, r, []byte{'x'}, 0, 0)
	assertReadFrame(t, r, nil, 0, 0)
	assertReadFrame(t, r, nil, 0)
	assertReadFrame(t, r, nil)
	if n := r.Len(); n > 0 {
		t.Fatalf("%v trailing bytes left in buffer: %s", n, r.String())
	}
}
