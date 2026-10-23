package wrpc_test

import (
	"bufio"
	"context"
	"io"
	"strings"
	"testing"

	wrpc "wrpc.io/go"
)

func TestFrameStreamReader(t *testing.T) {
	t.Run("empty frame", func(t *testing.T) {
		t.Parallel()
		ctx := context.Background()
		var src sink
		for _, frame := range []wrpc.Frame{
			{Data: []byte("hello")},
			{Path: []uint32{0}, Data: []byte("world")},
			{Path: []uint32{0}},
			{Path: []uint32{0}, Data: []byte("!")},
		} {
			if _, err := wrpc.WriteFrame(frame, &src); err != nil {
				t.Fatalf("failed to write frame: %s", err)
			}
		}

		r := wrpc.NewFrameStreamReader(ctx, &src, wrpc.NewSubscribePath().Index(0))
		sub, err := r.Index(0)
		if err != nil {
			t.Fatalf("failed to index reader: %s", err)
		}
		buf, err := io.ReadAll(r)
		if err == nil || !strings.Contains(err.Error(), "closed path") {
			t.Fatalf("expected the stream to fail, got `%v`", err)
		}
		if string(buf) != "hello" {
			t.Fatalf("expected `hello`, got `%s`", buf)
		}
		buf, err = io.ReadAll(sub)
		if err != nil {
			t.Fatalf("expected empty frame to close the path, got `%v`", err)
		}
		if string(buf) != "world" {
			t.Fatalf("expected `world`, got `%s`", buf)
		}
		assertClose(t, sub)
		assertClose(t, r)
	})

	t.Run("closed reader", func(t *testing.T) {
		t.Parallel()
		ctx := context.Background()
		pr, pw := io.Pipe()
		r := wrpc.NewFrameStreamReader(ctx, struct {
			*bufio.Reader
			io.Closer
		}{bufio.NewReader(pr), pr},
			wrpc.NewSubscribePath().Index(0),
			wrpc.NewSubscribePath().Index(1),
		)
		for _, i := range []uint32{0, 1} {
			sub, err := r.Index(i)
			if err != nil {
				t.Fatalf("failed to index reader: %s", err)
			}
			assertClose(t, sub)
		}
		var src sink
		for _, frame := range []wrpc.Frame{
			{Path: []uint32{0}},
			{Data: []byte("hello")},
			{Path: []uint32{1}, Data: []byte("world")},
		} {
			if _, err := wrpc.WriteFrame(frame, &src); err != nil {
				t.Fatalf("failed to write frame: %s", err)
			}
		}
		go func() {
			_, err := src.WriteTo(pw)
			pw.CloseWithError(err)
		}()
		buf, err := io.ReadAll(r)
		if err == nil || !strings.Contains(err.Error(), "closed reader") {
			t.Fatalf("expected the stream to fail, got `%v`", err)
		}
		if string(buf) != "hello" {
			t.Fatalf("expected `hello`, got `%s`", buf)
		}
		assertClose(t, r)
	})
}
