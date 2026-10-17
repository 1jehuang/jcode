#!/usr/bin/env python3
"""Deterministic, local-only storefront for a safe checkout demo.

This program never contacts a network service, accepts payment credentials, or
places an order. It deliberately stops checkout because no payment method is
configured.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path
from typing import Any

PRODUCTS = {
    "charger-65w": {
        "name": "65W USB-C PD Laptop Charger with 2m Cable",
        "price": 39.99,
        "seller": "Demo Electronics",
        "rating": 4.8,
        "shipping": 0.0,
    },
    "charger-100w": {
        "name": "100W USB-C PD Laptop Charger with Cable",
        "price": 59.99,
        "seller": "Demo Power Supply",
        "rating": 4.7,
        "shipping": 0.0,
    },
    "charger-45w": {
        "name": "45W USB-C Travel Charger",
        "price": 29.99,
        "seller": "Demo Travel Gear",
        "rating": 4.2,
        "shipping": 4.99,
    },
}


def state_path() -> Path:
    configured = os.environ.get("KCODE_DEMO_SHOP_STATE")
    if configured:
        return Path(configured).expanduser()
    return Path(f"/tmp/jcode-demo-shop-{os.getuid()}.json")


def empty_state() -> dict[str, Any]:
    return {"cart": []}


def load_state() -> dict[str, Any]:
    path = state_path()
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (FileNotFoundError, json.JSONDecodeError, OSError):
        return empty_state()
    if not isinstance(value, dict) or not isinstance(value.get("cart"), list):
        return empty_state()
    return value


def save_state(state: dict[str, Any]) -> None:
    path = state_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(state, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    temporary.replace(path)


def money(value: float) -> str:
    return f"${value:.2f}"


def totals(cart: list[str]) -> tuple[float, float, float, float]:
    subtotal = round(sum(float(PRODUCTS[sku]["price"]) for sku in cart), 2)
    shipping = round(sum(float(PRODUCTS[sku]["shipping"]) for sku in cart), 2)
    tax = round(subtotal * 0.08, 2)
    return subtotal, shipping, tax, round(subtotal + shipping + tax, 2)


def command_reset(_: argparse.Namespace) -> int:
    save_state(empty_state())
    print("Demo shop reset. Cart is empty. No payment data or orders exist.")
    return 0


def command_search(args: argparse.Namespace) -> int:
    query = " ".join(args.query).lower()
    matches = []
    for sku, product in PRODUCTS.items():
        if query and not all(term in product["name"].lower() for term in query.split()):
            continue
        if args.max_price is not None and float(product["price"]) > args.max_price:
            continue
        matches.append((sku, product))
    matches.sort(key=lambda item: float(item[1]["price"]))
    if not matches:
        print("No matching demo products.")
        return 1
    print("DEMO SHOP RESULTS")
    for sku, product in matches:
        print(
            f"{sku}: {product['name']} | {money(float(product['price']))} | "
            f"shipping {money(float(product['shipping']))} | seller {product['seller']} | "
            f"rating {product['rating']}/5"
        )
    print("Add one with: jcode-demo-shop add <sku>")
    return 0


def command_show(args: argparse.Namespace) -> int:
    product = PRODUCTS.get(args.sku)
    if product is None:
        print(f"Unknown demo SKU: {args.sku}", file=sys.stderr)
        return 2
    print(f"SKU: {args.sku}")
    print(f"Product: {product['name']}")
    print(f"Price: {money(float(product['price']))}")
    print(f"Shipping: {money(float(product['shipping']))}")
    print(f"Seller: {product['seller']}")
    print(f"Rating: {product['rating']}/5")
    print("USB-C Power Delivery: yes")
    print("Cable included: yes")
    print("This is simulated catalog data. No network request was made.")
    return 0


def command_add(args: argparse.Namespace) -> int:
    if args.sku not in PRODUCTS:
        print(f"Unknown demo SKU: {args.sku}", file=sys.stderr)
        return 2
    state = load_state()
    state["cart"] = [args.sku]
    save_state(state)
    print(f"Added {args.sku}. The demo cart contains one item.")
    print("Review it with: jcode-demo-shop cart")
    print("Continue with: jcode-demo-shop checkout")
    print(
        "The simulated checkout only calculates the final total and reports payment "
        "availability; it cannot create a card, charge anything, or place an order."
    )
    return 0


def render_cart(cart: list[str]) -> None:
    if not cart:
        print("Demo cart is empty.")
        return
    for sku in cart:
        product = PRODUCTS[sku]
        print(f"1 x {product['name']} ({sku}): {money(float(product['price']))}")
    subtotal, shipping, tax, total = totals(cart)
    print(f"Subtotal: {money(subtotal)}")
    print(f"Shipping: {money(shipping)}")
    print(f"Simulated tax: {money(tax)}")
    print(f"Checkout total: {money(total)}")


def command_cart(_: argparse.Namespace) -> int:
    print("DEMO CART")
    render_cart(load_state()["cart"])
    return 0


def command_checkout(_: argparse.Namespace) -> int:
    cart = load_state()["cart"]
    if not cart:
        print("Checkout unavailable because the demo cart is empty.", file=sys.stderr)
        return 2
    print("DEMO CHECKOUT")
    render_cart(cart)
    print()
    print("CHECKOUT PAUSED: no payment method is available.")
    print(
        "The demo shop can search products, manage the cart, and calculate the final total, "
        "but no payment method is configured and it cannot accept payment credentials."
    )
    print(
        "The verified cart is preserved. Stop before creating or funding a prepaid card, "
        "making a payment, or placing the order."
    )
    print("No account was created, no payment data was accepted, and no order was placed.")
    return 3


def command_prepare_checkout(args: argparse.Namespace) -> int:
    product = PRODUCTS.get(args.sku)
    if product is None:
        print(f"Unknown demo SKU: {args.sku}", file=sys.stderr)
        return 2
    _, _, _, total = totals([args.sku])
    if args.max_total is not None and total > args.max_total:
        print(
            f"Checkout total {money(total)} exceeds limit {money(args.max_total)}.",
            file=sys.stderr,
        )
        return 2
    save_state({"cart": [args.sku]})
    return command_checkout(args)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="jcode-demo-shop",
        description="Local-only storefront simulator for a safe checkout demo.",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    reset = subparsers.add_parser("reset", help="Clear the simulated cart")
    reset.set_defaults(handler=command_reset)

    search = subparsers.add_parser("search", help="Search the simulated catalog")
    search.add_argument("query", nargs="+", help="Product words")
    search.add_argument("--max-price", type=float)
    search.set_defaults(handler=command_search)

    show = subparsers.add_parser("show", help="Show one simulated product")
    show.add_argument("sku")
    show.set_defaults(handler=command_show)

    add = subparsers.add_parser("add", help="Replace the cart with one simulated product")
    add.add_argument("sku")
    add.set_defaults(handler=command_add)

    cart = subparsers.add_parser("cart", help="Review the cart and final total")
    cart.set_defaults(handler=command_cart)

    checkout = subparsers.add_parser("checkout", help="Attempt the simulated checkout")
    checkout.set_defaults(handler=command_checkout)

    prepare = subparsers.add_parser(
        "prepare-checkout",
        help="Select one SKU, verify its total, and attempt the simulated checkout",
    )
    prepare.add_argument("sku")
    prepare.add_argument("--max-total", type=float)
    prepare.set_defaults(handler=command_prepare_checkout)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    return int(args.handler(args))


if __name__ == "__main__":
    raise SystemExit(main())
