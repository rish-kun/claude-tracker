package httpapi

import (
	"html/template"
	"net/http"
)

var pageTmpl = template.Must(template.New("page").Parse(`<!doctype html>
<html><head><meta charset="utf-8"><title>cctrack — {{.Title}}</title>
<style>body{font-family:system-ui,sans-serif;max-width:36rem;margin:4rem auto;padding:0 1rem;line-height:1.5}
code{display:block;font-size:1.4rem;padding:1rem;background:#f3f3f3;border-radius:8px;word-break:break-all;user-select:all}</style>
</head><body><h1>{{.Title}}</h1><p>{{.Message}}</p>{{if .Code}}<code>{{.Code}}</code>{{end}}</body></html>`))

func htmlPage(w http.ResponseWriter, status int, title, message, code string) {
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(status)
	_ = pageTmpl.Execute(w, struct{ Title, Message, Code string }{title, message, code})
}
