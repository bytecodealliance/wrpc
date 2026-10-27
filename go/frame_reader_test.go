package wrpc_test

import (
	"bufio"
	"bytes"
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

	t.Run("close twice", func(t *testing.T) {
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
		sub0, err := r.Index(0)
		if err != nil {
			t.Fatalf("failed to index reader: %s", err)
		}
		sub1, err := r.Index(1)
		if err != nil {
			t.Fatalf("failed to index reader: %s", err)
		}
		assertClose(t, r)
		assertClose(t, sub0)
		assertClose(t, sub0)
		var src sink
		for _, frame := range []wrpc.Frame{
			{Path: []uint32{0}},
			{Path: []uint32{1}, Data: []byte("hello")},
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
		buf, err := io.ReadAll(sub1)
		if err != nil {
			t.Fatalf("failed to read: %s", err)
		}
		if string(buf) != "helloworld" {
			t.Fatalf("expected `helloworld`, got `%s`", buf)
		}
		assertClose(t, sub1)
	})

	t.Run("wildcard elements", func(t *testing.T) {
		t.Parallel()
		ctx := context.Background()
		pr, pw := io.Pipe()
		r := wrpc.NewFrameStreamReader(ctx, struct {
			*bufio.Reader
			io.Closer
		}{bufio.NewReader(pr), pr}, wrpc.NewSubscribePath().Index(0).Wildcard())
		write := func(frame wrpc.Frame) {
			var buf bytes.Buffer
			if _, err := wrpc.WriteFrame(frame, &buf); err != nil {
				t.Errorf("failed to encode frame: %s", err)
			}
			if _, err := pw.Write(buf.Bytes()); err != nil {
				t.Errorf("failed to write frame: %s", err)
			}
		}
		write(wrpc.Frame{Path: []uint32{0, 0}, Data: []byte("x")})
		write(wrpc.Frame{Path: []uint32{0, 2}, Data: []byte("b")})
		sub, err := r.Index(0, 2)
		if err != nil {
			t.Fatalf("failed to index reader: %s", err)
		}
		buf := make([]byte, 1)
		if _, err := io.ReadFull(sub, buf); err != nil {
			t.Fatalf("failed to read: %s", err)
		}
		if string(buf) != "b" {
			t.Fatalf("expected `b`, got `%s`", buf)
		}
		for _, i := range []uint32{0, 1} {
			if _, err := r.Index(0, i); err == nil || !strings.Contains(err.Error(), "released") {
				t.Fatalf("expected indexing released element %d to fail, got `%v`", i, err)
			}
		}
		assertClose(t, sub)
		go func() {
			write(wrpc.Frame{Path: []uint32{0, 0}, Data: []byte("y")})
			pw.Close()
		}()
		if _, err := io.ReadAll(r); err == nil || !strings.Contains(err.Error(), "released element") {
			t.Fatalf("expected the stream to fail, got `%v`", err)
		}
		assertClose(t, r)
	})

	t.Run("unclaimed elements", func(t *testing.T) {
		t.Parallel()
		ctx := context.Background()
		pr, pw := io.Pipe()
		r := wrpc.NewFrameStreamReader(ctx, struct {
			*bufio.Reader
			io.Closer
		}{bufio.NewReader(pr), pr}, wrpc.NewSubscribePath().Wildcard())
		go func() {
			var buf bytes.Buffer
			for i := range uint32(64) {
				if _, err := wrpc.WriteFrame(wrpc.Frame{Path: []uint32{i + 1}, Data: []byte("x")}, &buf); err != nil {
					t.Errorf("failed to encode frame: %s", err)
				}
			}
			for _, frame := range []wrpc.Frame{
				{Path: []uint32{0}, Data: []byte("a")},
				{Path: []uint32{65}, Data: []byte("b")},
			} {
				if _, err := wrpc.WriteFrame(frame, &buf); err != nil {
					t.Errorf("failed to encode frame: %s", err)
				}
			}
			if _, err := pw.Write(buf.Bytes()); err != nil {
				t.Errorf("failed to write frames: %s", err)
			}
			pw.Close()
		}()
		for _, frame := range []struct {
			i    uint32
			data string
		}{{0, "a"}, {65, "b"}} {
			sub, err := r.Index(frame.i)
			if err != nil {
				t.Fatalf("failed to index reader: %s", err)
			}
			buf := make([]byte, 1)
			if _, err := io.ReadFull(sub, buf); err != nil {
				t.Fatalf("failed to read: %s", err)
			}
			if string(buf) != frame.data {
				t.Fatalf("expected `%s`, got `%s`", frame.data, buf)
			}
			assertClose(t, sub)
		}
		if _, err := io.ReadAll(r); err != nil {
			t.Fatalf("failed to read: %s", err)
		}
		assertClose(t, r)
	})
}
