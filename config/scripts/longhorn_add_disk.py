import longhorn
import sys


LONGHORN_URL = "http://localhost:8080/v1"

def add_disk(node_name, disk_name, mount_dir):
    client = longhorn.Client(url=LONGHORN_URL)
    node = client.by_id_node(node_name)

    print()


if __name__ == "__main__":
    node_name = sys.argv[1]
    disk_name = sys.argv[2]
    mount_dir = sys.argv[3]
    add_disk(node_name, disk_name, mount_dir)
