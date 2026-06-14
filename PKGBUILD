# Maintainer: Alex Oleshkevich <techsupport@investerra.ch>
pkgname=fastapi-lsp
pkgver=0.1.0
pkgrel=1
pkgdesc="Language server for FastAPI and Starlette"
arch=('x86_64' 'aarch64')
url="https://github.com/alexoleshkevich/fastapi-lsp"
license=('MIT')
depends=()
makedepends=('rust' 'cargo')
source=("$pkgname-$pkgver.tar.gz::$url/archive/refs/tags/v$pkgver.tar.gz")
sha256sums=('SKIP')

build() {
  cd "$pkgname-$pkgver"
  cargo build --release --locked
}

check() {
  cd "$pkgname-$pkgver"
  cargo test --release --locked
}

package() {
  cd "$pkgname-$pkgver"
  install -Dm755 "target/release/$pkgname" "$pkgdir/usr/bin/$pkgname"
  install -Dm644 LICENSE "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
  install -Dm644 README.md "$pkgdir/usr/share/doc/$pkgname/README.md"
}
