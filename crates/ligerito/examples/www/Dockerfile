FROM nginx:alpine
COPY nginx.conf /etc/nginx/conf.d/default.conf
COPY index.html style.css worker.js /usr/share/nginx/html/
COPY ligerito.js ligerito.d.ts ligerito_bg.wasm ligerito_bg.wasm.d.ts /usr/share/nginx/html/
COPY favicon.ico favicon-16x16.png favicon-32x32.png /usr/share/nginx/html/
COPY apple-touch-icon.png android-chrome-192x192.png android-chrome-512x512.png /usr/share/nginx/html/
COPY site.webmanifest benchmark.html /usr/share/nginx/html/
COPY snippets/ /usr/share/nginx/html/snippets/
EXPOSE 8080
