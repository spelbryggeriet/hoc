# Hosting on Command

`hoc` is a tool for easily deploying and managing your own home network cluster. It keeps track
of all the necessary files and configuration for you, so you spend less time on being a system
administrator and more time on developing services for your cluster.

## Installation

Run the following in your terminal:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/spelbryggeriet/hoc/master/scripts/init.sh | sh
```

This will install the binary to `~/.local/bin` and update your PATH environment variable.

# TODO: Everything below here needs to be updated / removed

# HomePi Hosting Tools
The purpose of this tool is to help deploy clusterizd HomePi applications to a
Raspberry Pi kubernetes cluster.

## Compiling
In order to compile a development version of the project, run the following:

```sh
cargo build
```

To get a release build, run:

```sh
cargo build --release
```

## Configuring Mac Pro Registry server
When setting up the Mac, set an appropriate strong password and disallow SSH
access in the System Preferences Sharing pane, since we will only access the
Ubuntu Server partition.

### rEFInd setup

Download rEFInd
(https://sourceforge.net/projects/refind/files/0.12.0/refind-bin-0.12.0.zip/download?use_mirror=netix&download=&failedmirror=deac-riga.dl.sourceforge.net):

```sh
curl -L -o ~/Downloads/refind-bin-0.12.0.zip https://downloads.sourceforge.net/project/refind/0.12.0/refind-bin-0.12.0.zip?r=https%3A%2F%2Fsourceforge.net%2Fprojects%2Frefind%2Ffiles%2F0.12.0%2Frefind-bin-0.12.0.zip%2Fdownload%3Fuse_mirror%3Dnetix%26download%3D%26failedmirror%3Ddeac-riga.dl.sourceforge.net&ts=1607671230
unzip ~/Downloads/refind-bin-0.12.0.zip
```

Install rEFInd (you might have to go through recovery mode for this):

```sh
cd ~/Downloads/refind-bin-0.12.0
./refind-install
```

### Install Ubuntu Server
Have a partition or drive ready for installing Ubuntu Server on. Have it at 50GB
at least. Have another partition (as large as possible), allocated for storage.

Download Ubuntu Server from:
https://releases.ubuntu.com/20.04.1/ubuntu-20.04.1-live-server-amd64.iso?_ga=2.206302355.1281129767.1607884066-2020785146.1607884066

Insert an **empty** USB stick and find the corresponding device file (should be
something like `rdiskN`):

```sh
diskutil list
```

Flash the USB drive with the Ubuntu image file:

```sh
sudo dd bs=1m if=~/Downloads/ubuntu-20.04.1-live-server-amd64.iso of=/dev/rdiskN
```

Reboot the computer and from rEFInd, boot the USB drive (from `/EFI/grub` or
something like that).

Install Ubuntu Server by following the steps, choosing a strong password, and
using the small partition (don't mistake it for the macOS partition) to format
with the XFS filesystem, to have the OS installed on (mounted at /), and the
large partition to have for the storage (mounted at /mnt/hdd). Also install
OpenSSH server to be able to remotely connect to it.

### Setup Ubuntu Server

Login to the Ubuntu Server by using SSH with the username and password created
during the installation phase.

Update apt packages:

```sh
sudo apt-get update
```

Install `net-tools` (contains `route` command):

```sh
sudo apt install -y net-tools
```

Install `nfs-common` for NFS mount helper program:

```sh
sudo apt install -y nfs-common
```

Install and configure Docker:

```sh
sudo apt install -y docker.io

cat <<EOF | sudo tee /etc/docker/daemon.json
{
  "exec-opts": ["native.cgroupdriver=systemd"],
  "log-driver": "json-file",
  "log-opts": {
    "max-size": "100m"
  },
  "storage-driver": "overlay2"
}
EOF

sudo systemctl enable docker
```

Change file permissions for `/mnt/hdd`:

```sh
sudo chmod 0777 /mnt/hdd
```

### Install Kubernetes

Setup IP tables:

```sh
cat <<EOF | sudo tee /etc/sysctl.d/k8s.conf
net.bridge.bridge-nf-call-ip6tables = 1
net.bridge.bridge-nf-call-iptables = 1
EOF

sudo sysctl --system
```

Add Kubernetes packages:

```sh
curl -s https://packages.cloud.google.com/apt/doc/apt-key.gpg | sudo apt-key add -

cat <<EOF | sudo tee /etc/apt/sources.list.d/kubernetes.list
deb https://apt.kubernetes.io/ kubernetes-xenial main
EOF
```

Install packages:

```sh
sudo apt update && sudo apt install -y kubelet kubeadm kubectl
sudo apt-mark hold kubelet kubeadm kubectl
```

### Configure Kubernetes

Disable swap by editing the `/etc/fstab` file and commenting out the line that says:

```
/swap.img	none	swap	sw	0	0
```

Reboot.

Initialize control plane:

```sh
TOKEN=$(sudo kubeadm token generate)
sudo kubeadm init --token=${TOKEN} --kubernetes-version=v1.20.1 --pod-network-cidr=10.1.0.0/16
```

Copy kube config:

```sh
mkdir -p $HOME/.kube
sudo cp -i /etc/kubernetes/admin.conf $HOME/.kube/config
sudo chown $(id -u):$(id -g) $HOME/.kube/config
```

Deploy Flannel:

```sh
cat <<EOT | kubectl apply -f -
---
apiVersion: policy/v1beta1
kind: PodSecurityPolicy
metadata:
  name: psp.flannel.unprivileged
  annotations:
    seccomp.security.alpha.kubernetes.io/allowedProfileNames: docker/default
    seccomp.security.alpha.kubernetes.io/defaultProfileName: docker/default
    apparmor.security.beta.kubernetes.io/allowedProfileNames: runtime/default
    apparmor.security.beta.kubernetes.io/defaultProfileName: runtime/default
spec:
  privileged: false
  volumes:
    - configMap
    - secret
    - emptyDir
    - hostPath
  allowedHostPaths:
    - pathPrefix: "/etc/cni/net.d"
    - pathPrefix: "/etc/kube-flannel"
    - pathPrefix: "/run/flannel"
  readOnlyRootFilesystem: false
  # Users and groups
  runAsUser:
    rule: RunAsAny
  supplementalGroups:
    rule: RunAsAny
  fsGroup:
    rule: RunAsAny
  # Privilege Escalation
  allowPrivilegeEscalation: false
  defaultAllowPrivilegeEscalation: false
  # Capabilities
  allowedCapabilities: ['NET_ADMIN']
  defaultAddCapabilities: []
  requiredDropCapabilities: []
  # Host namespaces
  hostPID: false
  hostIPC: false
  hostNetwork: true
  hostPorts:
  - min: 0
    max: 65535
  # SELinux
  seLinux:
    # SELinux is unused in CaaSP
    rule: 'RunAsAny'
---
kind: ClusterRole
apiVersion: rbac.authorization.k8s.io/v1
metadata:
  name: flannel
rules:
  - apiGroups: ['extensions']
    resources: ['podsecuritypolicies']
    verbs: ['use']
    resourceNames: ['psp.flannel.unprivileged']
  - apiGroups:
      - ""
    resources:
      - pods
    verbs:
      - get
  - apiGroups:
      - ""
    resources:
      - nodes
    verbs:
      - list
      - watch
  - apiGroups:
      - ""
    resources:
      - nodes/status
    verbs:
      - patch
---
kind: ClusterRoleBinding
apiVersion: rbac.authorization.k8s.io/v1
metadata:
  name: flannel
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: ClusterRole
  name: flannel
subjects:
- kind: ServiceAccount
  name: flannel
  namespace: kube-system
---
apiVersion: v1
kind: ServiceAccount
metadata:
  name: flannel
  namespace: kube-system
---
kind: ConfigMap
apiVersion: v1
metadata:
  name: kube-flannel-cfg
  namespace: kube-system
  labels:
    tier: node
    app: flannel
data:
  cni-conf.json: |
    {
      "name": "cbr0",
      "cniVersion": "0.3.1",
      "plugins": [
        {
          "type": "flannel",
          "delegate": {
            "hairpinMode": true,
            "isDefaultGateway": true
          }
        },
        {
          "type": "portmap",
          "capabilities": {
            "portMappings": true
          }
        }
      ]
    }
  net-conf.json: |
    {
      "Network": "10.1.0.0/16",
      "Backend": {
        "Type": "vxlan"
      }
    }
---
apiVersion: apps/v1
kind: DaemonSet
metadata:
  name: kube-flannel-ds-amd64
  namespace: kube-system
  labels:
    tier: node
    app: flannel
spec:
  selector:
    matchLabels:
      app: flannel
  template:
    metadata:
      labels:
        tier: node
        app: flannel
    spec:
      affinity:
        nodeAffinity:
          requiredDuringSchedulingIgnoredDuringExecution:
            nodeSelectorTerms:
              - matchExpressions:
                  - key: beta.kubernetes.io/os
                    operator: In
                    values:
                      - linux
                  - key: beta.kubernetes.io/arch
                    operator: In
                    values:
                      - amd64
      hostNetwork: true
      tolerations:
      - operator: Exists
        effect: NoSchedule
      serviceAccountName: flannel
      initContainers:
      - name: install-cni
        image: quay.io/coreos/flannel:v0.12.0-amd64
        command:
        - cp
        args:
        - -f
        - /etc/kube-flannel/cni-conf.json
        - /etc/cni/net.d/10-flannel.conflist
        volumeMounts:
        - name: cni
          mountPath: /etc/cni/net.d
        - name: flannel-cfg
          mountPath: /etc/kube-flannel/
      containers:
      - name: kube-flannel
        image: quay.io/coreos/flannel:v0.12.0-amd64
        command:
        - /opt/bin/flanneld
        args:
        - --ip-masq
        - --kube-subnet-mgr
        resources:
          requests:
            cpu: "100m"
            memory: "50Mi"
          limits:
            cpu: "100m"
            memory: "50Mi"
        securityContext:
          privileged: false
          capabilities:
            add: ["NET_ADMIN"]
        env:
        - name: POD_NAME
          valueFrom:
            fieldRef:
              fieldPath: metadata.name
        - name: POD_NAMESPACE
          valueFrom:
            fieldRef:
              fieldPath: metadata.namespace
        volumeMounts:
        - name: run
          mountPath: /run/flannel
        - name: flannel-cfg
          mountPath: /etc/kube-flannel/
      volumes:
        - name: run
          hostPath:
            path: /run/flannel
        - name: cni
          hostPath:
            path: /etc/cni/net.d
        - name: flannel-cfg
          configMap:
            name: kube-flannel-cfg
---
apiVersion: apps/v1
kind: DaemonSet
metadata:
  name: kube-flannel-ds-arm64
  namespace: kube-system
  labels:
    tier: node
    app: flannel
spec:
  selector:
    matchLabels:
      app: flannel
  template:
    metadata:
      labels:
        tier: node
        app: flannel
    spec:
      affinity:
        nodeAffinity:
          requiredDuringSchedulingIgnoredDuringExecution:
            nodeSelectorTerms:
              - matchExpressions:
                  - key: beta.kubernetes.io/os
                    operator: In
                    values:
                      - linux
                  - key: beta.kubernetes.io/arch
                    operator: In
                    values:
                      - arm64
      hostNetwork: true
      tolerations:
      - operator: Exists
        effect: NoSchedule
      serviceAccountName: flannel
      initContainers:
      - name: install-cni
        image: quay.io/coreos/flannel:v0.12.0-arm64
        command:
        - cp
        args:
        - -f
        - /etc/kube-flannel/cni-conf.json
        - /etc/cni/net.d/10-flannel.conflist
        volumeMounts:
        - name: cni
          mountPath: /etc/cni/net.d
        - name: flannel-cfg
          mountPath: /etc/kube-flannel/
      containers:
      - name: kube-flannel
        image: quay.io/coreos/flannel:v0.12.0-arm64
        command:
        - /opt/bin/flanneld
        args:
        - --ip-masq
        - --kube-subnet-mgr
        resources:
          requests:
            cpu: "100m"
            memory: "50Mi"
          limits:
            cpu: "100m"
            memory: "50Mi"
        securityContext:
          privileged: false
          capabilities:
             add: ["NET_ADMIN"]
        env:
        - name: POD_NAME
          valueFrom:
            fieldRef:
              fieldPath: metadata.name
        - name: POD_NAMESPACE
          valueFrom:
            fieldRef:
              fieldPath: metadata.namespace
        volumeMounts:
        - name: run
          mountPath: /run/flannel
        - name: flannel-cfg
          mountPath: /etc/kube-flannel/
      volumes:
        - name: run
          hostPath:
            path: /run/flannel
        - name: cni
          hostPath:
            path: /etc/cni/net.d
        - name: flannel-cfg
          configMap:
            name: kube-flannel-cfg
---
apiVersion: apps/v1
kind: DaemonSet
metadata:
  name: kube-flannel-ds-arm
  namespace: kube-system
  labels:
    tier: node
    app: flannel
spec:
  selector:
    matchLabels:
      app: flannel
  template:
    metadata:
      labels:
        tier: node
        app: flannel
    spec:
      affinity:
        nodeAffinity:
          requiredDuringSchedulingIgnoredDuringExecution:
            nodeSelectorTerms:
              - matchExpressions:
                  - key: beta.kubernetes.io/os
                    operator: In
                    values:
                      - linux
                  - key: beta.kubernetes.io/arch
                    operator: In
                    values:
                      - arm
      hostNetwork: true
      tolerations:
      - operator: Exists
        effect: NoSchedule
      serviceAccountName: flannel
      initContainers:
      - name: install-cni
        image: quay.io/coreos/flannel:v0.12.0-arm
        command:
        - cp
        args:
        - -f
        - /etc/kube-flannel/cni-conf.json
        - /etc/cni/net.d/10-flannel.conflist
        volumeMounts:
        - name: cni
          mountPath: /etc/cni/net.d
        - name: flannel-cfg
          mountPath: /etc/kube-flannel/
      containers:
      - name: kube-flannel
        image: quay.io/coreos/flannel:v0.12.0-arm
        command:
        - /opt/bin/flanneld
        args:
        - --ip-masq
        - --kube-subnet-mgr
        resources:
          requests:
            cpu: "100m"
            memory: "50Mi"
          limits:
            cpu: "100m"
            memory: "50Mi"
        securityContext:
          privileged: false
          capabilities:
             add: ["NET_ADMIN"]
        env:
        - name: POD_NAME
          valueFrom:
            fieldRef:
              fieldPath: metadata.name
        - name: POD_NAMESPACE
          valueFrom:
            fieldRef:
              fieldPath: metadata.namespace
        volumeMounts:
        - name: run
          mountPath: /run/flannel
        - name: flannel-cfg
          mountPath: /etc/kube-flannel/
      volumes:
        - name: run
          hostPath:
            path: /run/flannel
        - name: cni
          hostPath:
            path: /etc/cni/net.d
        - name: flannel-cfg
          configMap:
            name: kube-flannel-cfg
---
apiVersion: apps/v1
kind: DaemonSet
metadata:
  name: kube-flannel-ds-ppc64le
  namespace: kube-system
  labels:
    tier: node
    app: flannel
spec:
  selector:
    matchLabels:
      app: flannel
  template:
    metadata:
      labels:
        tier: node
        app: flannel
    spec:
      affinity:
        nodeAffinity:
          requiredDuringSchedulingIgnoredDuringExecution:
            nodeSelectorTerms:
              - matchExpressions:
                  - key: beta.kubernetes.io/os
                    operator: In
                    values:
                      - linux
                  - key: beta.kubernetes.io/arch
                    operator: In
                    values:
                      - ppc64le
      hostNetwork: true
      tolerations:
      - operator: Exists
        effect: NoSchedule
      serviceAccountName: flannel
      initContainers:
      - name: install-cni
        image: quay.io/coreos/flannel:v0.12.0-ppc64le
        command:
        - cp
        args:
        - -f
        - /etc/kube-flannel/cni-conf.json
        - /etc/cni/net.d/10-flannel.conflist
        volumeMounts:
        - name: cni
          mountPath: /etc/cni/net.d
        - name: flannel-cfg
          mountPath: /etc/kube-flannel/
      containers:
      - name: kube-flannel
        image: quay.io/coreos/flannel:v0.12.0-ppc64le
        command:
        - /opt/bin/flanneld
        args:
        - --ip-masq
        - --kube-subnet-mgr
        resources:
          requests:
            cpu: "100m"
            memory: "50Mi"
          limits:
            cpu: "100m"
            memory: "50Mi"
        securityContext:
          privileged: false
          capabilities:
             add: ["NET_ADMIN"]
        env:
        - name: POD_NAME
          valueFrom:
            fieldRef:
              fieldPath: metadata.name
        - name: POD_NAMESPACE
          valueFrom:
            fieldRef:
              fieldPath: metadata.namespace
        volumeMounts:
        - name: run
          mountPath: /run/flannel
        - name: flannel-cfg
          mountPath: /etc/kube-flannel/
      volumes:
        - name: run
          hostPath:
            path: /run/flannel
        - name: cni
          hostPath:
            path: /etc/cni/net.d
        - name: flannel-cfg
          configMap:
            name: kube-flannel-cfg
---
apiVersion: apps/v1
kind: DaemonSet
metadata:
  name: kube-flannel-ds-s390x
  namespace: kube-system
  labels:
    tier: node
    app: flannel
spec:
  selector:
    matchLabels:
      app: flannel
  template:
    metadata:
      labels:
        tier: node
        app: flannel
    spec:
      affinity:
        nodeAffinity:
          requiredDuringSchedulingIgnoredDuringExecution:
            nodeSelectorTerms:
              - matchExpressions:
                  - key: beta.kubernetes.io/os
                    operator: In
                    values:
                      - linux
                  - key: beta.kubernetes.io/arch
                    operator: In
                    values:
                      - s390x
      hostNetwork: true
      tolerations:
      - operator: Exists
        effect: NoSchedule
      serviceAccountName: flannel
      initContainers:
      - name: install-cni
        image: quay.io/coreos/flannel:v0.12.0-s390x
        command:
        - cp
        args:
        - -f
        - /etc/kube-flannel/cni-conf.json
        - /etc/cni/net.d/10-flannel.conflist
        volumeMounts:
        - name: cni
          mountPath: /etc/cni/net.d
        - name: flannel-cfg
          mountPath: /etc/kube-flannel/
      containers:
      - name: kube-flannel
        image: quay.io/coreos/flannel:v0.12.0-s390x
        command:
        - /opt/bin/flanneld
        args:
        - --ip-masq
        - --kube-subnet-mgr
        resources:
          requests:
            cpu: "100m"
            memory: "50Mi"
          limits:
            cpu: "100m"
            memory: "50Mi"
        securityContext:
          privileged: false
          capabilities:
             add: ["NET_ADMIN"]
        env:
        - name: POD_NAME
          valueFrom:
            fieldRef:
              fieldPath: metadata.name
        - name: POD_NAMESPACE
          valueFrom:
            fieldRef:
              fieldPath: metadata.namespace
        volumeMounts:
        - name: run
          mountPath: /run/flannel
        - name: flannel-cfg
          mountPath: /etc/kube-flannel/
      volumes:
        - name: run
          hostPath:
            path: /run/flannel
        - name: cni
          hostPath:
            path: /etc/cni/net.d
        - name: flannel-cfg
          configMap:
            name: kube-flannel-cfg
EOT
```

<!-- Join cluster:

```sh
# on control plane node
kubeadm token create --print-join-command

# on registry node
sudo kubeadm join "<control node IP>:6443" --token "<token value>" --discovery-token-ca-cert-hash "<sha256 value>"
```

Copy kube config to `~/.kube/config` on registry server and update file permissions:

```sh
chmod 600 $HOME/.kube/config
```

Label and taint node:

```sh
kubectl label node homepi-registry-node node-role.kubernetes.io/registry=""
kubectl taint node homepi-registry-node node-role.kubernetes.io/registry:NoSchedule

As root, edit `/etc/systemd/system/kubelet.service.d/10-kubeadm.conf`, and add
the following line:

```sh
Environment="KUBELET_EXTRA_ARGS=--resolv-conf=/etc/resolv.conf"
```

Run:

```sh
sudo systemctl daemon-reload
sudo service kubelet restart
```



``` -->

Create local storage Storage Class:

```sh
cat <<EOT | kubectl create -f -
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: local-storage
provisioner: kubernetes.io/no-provisioner
volumeBindingMode: WaitForFirstConsumer
EOT
```

Create NFS storage Storage Class:

```sh
cat <<EOT | kubectl create -f -
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: nfs-storage
provisioner: kubernetes.io/no-provisioner
volumeBindingMode: WaitForFirstConsumer
EOT
```

Create local storage Persistent Volume for nfs-server:

```sh
cat <<EOT | kubectl create -f -
apiVersion: v1
kind: PersistentVolume
metadata:
  name: nfs-server
  labels:
    app.kubernetes.io/name: nfs-server

spec:
  capacity:
    storage: 1900G
  volumeMode: Filesystem
  accessModes:
    - ReadWriteOnce
  persistentVolumeReclaimPolicy: Retain
  storageClassName: local-storage
  local:
    path: "/mnt/hdd"
  nodeAffinity:
    required:
      nodeSelectorTerms:
      - matchExpressions:
        - key: node-role.kubernetes.io/master
          operator: Exists
EOT
```

Create PostgreSQL Persistent Volume:

```sh
sudo mkdir -m 0777 /mnt/hdd/postgresql
cat <<EOT | kubectl create -f -
apiVersion: v1
kind: PersistentVolume
metadata:
  name: postgresql
  labels:
    app.kubernetes.io/name: postgresql

spec:
  capacity:
    storage: 50G
  volumeMode: Filesystem
  accessModes:
    - ReadWriteMany
  persistentVolumeReclaimPolicy: Retain
  storageClassName: nfs-storage
  nfs:
    server: 10.96.16.128
    path: /srv/nfs/postgresql
EOT
```

Create Vault Persistent Volumes:

```sh
sudo mkdir -p -m 0777 /mnt/hdd/secrets/data /mnt/hdd/secrets/audit

cat <<EOT | kubectl create -f -
apiVersion: v1
kind: PersistentVolume
metadata:
  name: vault-data
  labels:
    app.kubernetes.io/name: vault

spec:
  claimRef:
    name: data-vault-0
    namespace: vault
  capacity:
    storage: 10G
  volumeMode: Filesystem
  accessModes:
    - ReadWriteMany
  persistentVolumeReclaimPolicy: Retain
  storageClassName: nfs-storage
  nfs:
    server: 10.96.16.128
    path: /srv/nfs/secrets/data
EOT

cat <<EOT | kubectl create -f -
apiVersion: v1
kind: PersistentVolume
metadata:
  name: vault-audit
  labels:
    app.kubernetes.io/name: vault

spec:
  claimRef:
    name: audit-vault-0
    namespace: vault
  capacity:
    storage: 10G
  volumeMode: Filesystem
  accessModes:
    - ReadWriteMany
  persistentVolumeReclaimPolicy: Retain
  storageClassName: nfs-storage
  nfs:
    server: 10.96.16.128
    path: /srv/nfs/secrets/audit
EOT
```

### Install dependencies

Install Helm:

```sh
curl https://baltocdn.com/helm/signing.asc | sudo apt-key add -
sudo apt-get install -y apt-transport-https
echo "deb https://baltocdn.com/helm/stable/debian/ all main" | sudo tee /etc/apt/sources.list.d/helm-stable-debian.list
sudo apt update && sudo apt install -y helm
```

Add Helm repositories:

```sh
helm repo add chartmuseum https://chartmuseum.github.io/charts
helm repo add bitnami https://charts.bitnami.com/bitnami
helm repo add traefik https://helm.traefik.io/traefik
helm repo add jetstack https://charts.jetstack.io
helm repo add hashicorp https://helm.releases.hashicorp.com
```

Install cert-manager:

```sh
kubectl create namespace cert-manager
helm install cert-manager jetstack/cert-manager \
    --namespace cert-manager \
    --set tolerations\[0\].key=node-role.kubernetes.io/master \
    --set tolerations\[0\].operator=Exists \
    --set nodeSelector.beta\\.kubernetes\\.io/arch=amd64 \
    --set installCRDs=true
```

Create cluster issuer for certificate creation:

```sh
cat <<EOT | kubectl create -f -
apiVersion: cert-manager.io/v1
kind: ClusterIssuer
metadata:
  name: letsencrypt
spec:
  acme:
    email: hampuslidin@gmail.com
    server: https://acme-v02.api.letsencrypt.org/directory
    preferredChain: "ISRG Root X1"
    privateKeySecretRef:
      name: letsencrypt-account-key
    solvers:
      - http01:
          ingress:
            class: traefik
EOT
```

Install vault:

```sh
kubectl create namespace vault

cat <<EOT | kubectl create -f -
kind: Certificate
apiVersion: cert-manager.io/v1
metadata:
  name: secrets-homepi-cert
  namespace: vault

spec:
  dnsNames:
    - secrets.homepi.se
  secretName: secrets-homepi-cert-tls
  issuerRef:
    name: letsencrypt
    kind: ClusterIssuer
  privateKey:
    rotationPolicy: Always
EOT

helm install vault hashicorp/vault \
    --namespace vault \
    --set injector.tolerations="$(echo -e "- key: node-role.kubernetes.io/master\n  operator: Exists")" \
    --set injector.nodeSelector="beta.kubernetes.io/arch: amd64" \
    --set injector.resources.requests.memory=256Mi \
    --set injector.resources.requests.cpu=250m \
    --set injector.resources.limits.memory=256Mi \
    --set injector.resources.limits.cpu=250m \
    --set server.tolerations="$(echo -e "- key: node-role.kubernetes.io/master\n  operator: Exists")" \
    --set server.nodeSelector="beta.kubernetes.io/arch: amd64" \
    --set server.resources.requests.memory=256Mi \
    --set server.resources.requests.cpu=250m \
    --set server.resources.limits.memory=256Mi \
    --set server.resources.limits.cpu=250m \
    --set server.dataStorage.size=10G \
    --set server.dataStorage.storageClass=nfs-storage \
    --set server.dataStorage.accessMode=ReadWriteMany \
    --set server.auditStorage.enabled=true \
    --set server.auditStorage.size=10G \
    --set server.auditStorage.storageClass=nfs-storage \
    --set server.auditStorage.accessMode=ReadWriteMany \
    --set server.ingress.enabled=true \
    --set server.ingress.annotations.kubernetes\\.io/ingress\\.class=traefik \
    --set server.ingress.hosts\[0\].host=secrets.homepi.se \
    --set server.ingress.hosts\[0\].paths={/} \
    --set server.ingress.tls\[0\].hosts={secrets.homepi.se} \
    --set server.ingress.tls\[0\].secretName=secrets-homepi-cert-tls \
    --set ui.enabled=true
```

Go to https://secrets.homepi.se. Enter 5 in the Key shares and 3 in the Key
threshold text fields. Click Initialize. When the unseal keys are presented,
scroll down to the bottom and select Download key. Save the generated unseal
keys file to your computer. The unseal process requires these keys and the
access requires the root token. Store the keys (not keys_base64) somewhere safe.
Click Continue to Unseal to proceed. Copy one of the keys and enter it in the
Master Key Portion field. Click Unseal to proceed. Repeat with two other keys.

Install Chartmuseum:

```sh
sudo mkdir -m 0777 /mnt/hdd/charts
kubectl create namespace chartmuseum

cat <<EOT | kubectl create -f -
kind: Certificate
apiVersion: cert-manager.io/v1
metadata:
  name: charts-homepi-cert
  namespace: chartmuseum

spec:
  dnsNames:
    - charts.homepi.se
  secretName: charts-homepi-cert-tls
  issuerRef:
    name: letsencrypt
    kind: ClusterIssuer
  privateKey:
    rotationPolicy: Always
EOT

helm install chartmuseum chartmuseum/chartmuseum \
    --namespace chartmuseum \
    --set tolerations\[0\].key=node-role.kubernetes.io/master \
    --set tolerations\[0\].operator=Exists \
    --set nodeSelector.beta\\.kubernetes\\.io/arch=amd64 \
    --set env.open.DISABLE_API=false \
    --set persistence.enabled=true \
    --set persistence.accessMode=ReadWriteMany \
    --set persistence.size=8G \
    --set persistence.volumeName=chartmuseum \
    --set persistence.pv.enabled=true \
    --set persistence.pv.pvname=chartmuseum \
    --set persistence.pv.capacity.storage=8G \
    --set persistence.pv.accessMode=ReadWriteMany \
    --set persistence.pv.nfs.server=10.96.16.128 \
    --set persistence.pv.nfs.path=/srv/nfs/charts \
    --set ingress.enabled=true \
    --set ingress.hosts\[0\].name=charts.homepi.se \
    --set ingress.hosts\[0\].tls=true \
    --set ingress.hosts\[0\].tlsSecret=charts-homepi-cert-tls
```

Install Postgres Helm chart:

```sh
CREATE_ROLES_SQL=`cat <<EOT | sed 's/,/\\\,/g'
CREATE ROLE "vault-root" SUPERUSER CREATEROLE LOGIN PASSWORD 'rootpassword';
CREATE ROLE admin CREATEDB LOGIN PASSWORD 'mypassword';
CREATE ROLE readwrite;
CREATE ROLE readonly;

GRANT readonly TO readwrite;
GRANT readwrite TO admin;

REVOKE ALL ON DATABASE postgres FROM PUBLIC;
REVOKE ALL ON SCHEMA public FROM PUBLIC;
GRANT CONNECT ON DATABASE postgres TO admin;

ALTER DEFAULT PRIVILEGES FOR ROLE admin
GRANT SELECT ON TABLES TO readonly;

ALTER DEFAULT PRIVILEGES FOR ROLE admin
GRANT INSERT, UPDATE, DELETE, TRUNCATE ON TABLES TO readwrite;

ALTER DEFAULT PRIVILEGES FOR ROLE admin
GRANT USAGE, SELECT, UPDATE ON SEQUENCES TO readwrite;
EOT
`

ALTER_TEMPLATE_SH=`cat <<'EOT'
#!/bin/bash
set -xe

psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname "template1" <<EOSQL
    REVOKE ALL ON SCHEMA public FROM PUBLIC;
    GRANT USAGE ON SCHEMA public TO readonly;
    GRANT CREATE ON SCHEMA public TO admin;

    ALTER DEFAULT PRIVILEGES FOR ROLE admin
    GRANT SELECT ON TABLES TO readonly;

    ALTER DEFAULT PRIVILEGES FOR ROLE admin
    GRANT INSERT, UPDATE, DELETE, TRUNCATE ON TABLES TO readwrite;

    ALTER DEFAULT PRIVILEGES FOR ROLE admin
    GRANT USAGE, SELECT, UPDATE ON SEQUENCES TO readwrite;
EOSQL
EOT
`

kubectl create namespace postgresql
helm install postgresql bitnami/postgresql \
    --namespace postgresql \
    --set fullnameOverride=master-instance \
    --set primary.tolerations\[0\].key=node-role.kubernetes.io/master \
    --set primary.tolerations\[0\].operator=Exists \
    --set primary.nodeSelector.beta\\.kubernetes\\.io/arch=amd64 \
    --set persistence.size=8G \
    --set persistence.accessModes={ReadWriteMany} \
    --set persistence.storageClass=nfs-storage \
    --set persistence.selector.matchLabels.app\\.kubernetes\\.io/name=postgresql \
    --set initdbScripts.010-create_roles\\.sql="$CREATE_ROLES_SQL" \
    --set initdbScripts.020-alter_template\\.sh="$ALTER_TEMPLATE_SH"

kubectl \
    -n postgresql \
    exec -it master-instance-0 -- \
        env PGPASSWORD=`kubectl \
                -n postgresql \
                -o jsonpath='{.data.postgresql-password}' \
                get secrets master-instance \
                | base64 -D` \
            psql \
                -U postgres \
                template1 \
                -c "$TEMPLATE_SQL"
```

Install Traefik ingress controller:

```sh
kubectl create namespace ingress-controller
helm install traefik traefik/traefik \
    --namespace ingress-controller \
    --set tolerations\[0\].key=node-role.kubernetes.io/master \
    --set tolerations\[0\].operator=Exists \
    --set nodeSelector.beta\\.kubernetes\\.io/arch=amd64 \
    --set logs.access.enabled=true \
    --set service.type=NodePort \
    --set ports.web.nodePort=30080 \
    --set ports.web.redirectTo=websecure \
    --set ports.websecure.nodePort=30443 \
    --set ports.websecure.tls.enabled=true \
    --set ingressClass.enabled=true \
    --set ingressClass.isDefaultClass=true \
    --set additionalArguments\[0\]=--providers.kubernetesIngress.ingressClass=traefik
```

## Setup other infra

```sh
USER_NAME='lidin'
PASSWORD='<enter password>'
LOGIN_TOKEN=`cat <<EOT | jq -c \
    | curl -s -X POST -d @- https://secrets.homepi.se/v1/auth/userpass/login/$USER_NAME \
    | jq -r '.auth.client_token'
{
    "password": "$PASSWORD"
}
EOT
`

cat <<EOT | jq -c \
    | curl -s -H "X-Vault-Token: $LOGIN_TOKEN" -X POST -d @- https://secrets.homepi.se/v1/database/config/postgres
{
    "plugin_name": "postgresql-database-plugin",
    "connection_url": "postgresql://{{username}}:{{password}}@master-instance.postgresql.svc.cluster.local:5432/postgres?sslmode=disable",
    "allowed_roles": ["readonly", "readwrite", "admin"],
    "username": "vault-root",
    "password": "rootpassword"
}
EOT

curl -s -H "X-Vault-Token: $LOGIN_TOKEN" -X POST https://secrets.homepi.se/v1/database/rotate-root/postgres

cat <<EOT | jq -c \
    | curl -s -H "X-Vault-Token: $LOGIN_TOKEN" -X POST -d @- https://secrets.homepi.se/v1/database/roles/readonly
{
    "db_name": "postgres",
    "creation_statements": [
        "CREATE ROLE \"{{name}}\" WITH LOGIN PASSWORD '{{password}}' VALID UNTIL '{{expiration}}' INHERIT;",
        "GRANT readonly TO \"{{name}}\";"
    ],
    "default_ttl": "1h",
    "max_ttl": "24h"
}
EOT

cat <<EOT | jq -c \
    | curl -s -H "X-Vault-Token: $LOGIN_TOKEN" -X POST -d @- https://secrets.homepi.se/v1/database/roles/readwrite
{
    "db_name": "postgres",
    "creation_statements": [
        "CREATE ROLE \"{{name}}\" WITH LOGIN PASSWORD '{{password}}' VALID UNTIL '{{expiration}}' INHERIT;",
        "GRANT readwrite TO \"{{name}}\";"
    ],
    "default_ttl": "1h",
    "max_ttl": "24h"
}
EOT

cat <<EOT | jq -c \
    | curl -s -H "X-Vault-Token: $LOGIN_TOKEN" -X POST -d @- https://secrets.homepi.se/v1/database/static-roles/admin
{
    "db_name": "postgres",
    "rotation_statements": ["ALTER USER \"{{name}}\" WITH PASSWORD '{{password}}';"],
    "username": "admin",
    "rotation_period": "24h"
}
EOT

cat <<EOT | jq -c \
    | curl -s -H "X-Vault-Token: $LOGIN_TOKEN" -X PUT -d @- https://secrets.homepi.se/v1/sys/policies/acl/db-readonly
{
  "policy": "path \"database/creds/readonly\" { capabilities = [ \"read\" ] }"
}
EOT

cat <<EOT | jq -c \
    | curl -s -H "X-Vault-Token: $LOGIN_TOKEN" -X PUT -d @- https://secrets.homepi.se/v1/sys/policies/acl/db-readwrite
{
  "policy": "path \"database/creds/readwrite\" { capabilities = [ \"read\" ] }"
}
EOT

cat <<EOT | jq -c \
    | curl -s -H "X-Vault-Token: $LOGIN_TOKEN" -X PUT -d @- https://secrets.homepi.se/v1/sys/policies/acl/db-admin
{
  "policy": "path \"database/static-creds/admin\" { capabilities = [ \"read\" ] }"
}
EOT
```
